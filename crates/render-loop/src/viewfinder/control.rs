//! Управление видоискателем на ходу: чем рисовать и какие стадии ISP включены.
//!
//! Команды идут тем же путём, что события surface: маршрутизатор кладёт их в `RenderInbox`
//! ([`crate::mailbox::RenderInbox::viewfinder_command`]), поток рендера применяет их между кадрами
//! и записывает итог в [`crate::thread::RenderStatus::viewfinder_commands`]. Команды меняют только
//! настройки видоискателя; сами кадры, лизинги и fence-ы остаются за бэкендом.
//!
//! Зачем это нужно: A/B-замер «стадия → мс GPU → мА» (решение 45) переключает набор стадий и
//! рисовальщик (строка «камера без ISP» = отладочная мозаика 05) без перезапуска показа.

use camera_contract::viewfinder_crop::{CropAspect, ViewfinderCrop};
use std::sync::Arc;

use isp_vulkan::viewfinder::IspStageSet;
use look_contract::{LookSettings, ToneBaseModel};
use scope_contract::ScopeIntent;
use serde::{Deserialize, Serialize};

use super::framing::ViewfinderFraming;
use super::headroom::HeadroomBench;

/// Чем видоискатель рисует кадры камеры.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewfinderPainter {
    /// Продукт: ISP видоискателя (слитый compute-проход) + компоновщик на экран.
    #[default]
    Isp,
    /// Отладочная мозаика сессии 05 (без ISP). Нужна как база «камера без ISP» в таблице
    /// «стадия → мс GPU → мА» (решение 45). Значения пишутся под sRGB: на экране Display P3
    /// цвета мозаики не точны — это отладочный вид, а не продукт.
    DebugMosaic,
}

/// Команда видоискателю. Каждая описывает намерение целиком (никаких «флагов»). Не `Copy`: кадрирование по
/// зуму держит общий каталог шкал камер (`Arc`).
#[derive(Debug, Clone, PartialEq)]
pub enum ViewfinderCommand {
    /// Рисовать выбранным рисовальщиком.
    SetPainter(ViewfinderPainter),
    /// Включить ровно этот набор стадий ISP (A/B-замер цены стадии; продукт — `IspStageSet::ALL`).
    SetIspStages(IspStageSet),
    /// Кадрировать видоискатель так (соотношение сторон и зум, по центру полезной области).
    SetCrop(ViewfinderCrop),
    /// Кадрирование И место на экране одной транзакцией (решение 93): ни одного кадра «новое кадрирование в
    /// старом месте». `SetCrop` при этом по-прежнему меняет только кадрирование и места не трогает.
    SetFraming(ViewfinderFraming),
    /// Включить или выключить стенд «запас под видео» (V4, см. [`super::headroom`]). Не продукт:
    /// включается только на время замера; видоискатель при этом работает как обычно.
    SetHeadroomBench(HeadroomBench),
    /// Вид картинки видоискателя: ВСЕ значения ручек разом (решение 178), а не «что поменялось». Значения
    /// общие (`Arc`): исполнитель эффектов держит копию у себя, команда ничего не копирует по дороге.
    /// Повтор тех же значений — `AlreadyInEffect`. Снимок эту команду не использует: он берёт вид из плана.
    SetLook(Arc<LookSettings>),
    /// Модель основы тона видоискателя (решение 195: основа — часть вида, значения ручек её не содержат). Меняется
    /// на ходу без паузы: варианты шейдера всех моделей заготовлены заранее (`viewfinder.isp.prewarm_tone_bases`).
    /// Повтор той же модели — `AlreadyInEffect`. Снимок эту команду не использует: он берёт основу из плана.
    SetToneBase(ToneBaseModel),
    /// Намерение экрана о цветовых приборах (14h): показать такой прибор такого размера или скрыть. `Show` заменяет
    /// прежний запрос целиком; `Hide` при закрытых приборах — `AlreadyInEffect`. Размер картинки сверх предела
    /// конфига (`viewfinder.scopes.max_picture_side_px`) — `Rejected` (предел знает только бэкенд, не эта проверка).
    /// Запрос действует, пока источник UI установлен: его уход отзывает запрос (см. `ViewfinderSettings`).
    ///
    /// # Контракт порядка (решение 190)
    /// Приборы считаются ТОЛЬКО пока установлен источник UI: счёт и рисование картинки управляются одним состоянием.
    /// Поэтому `Show` при неустановленном источнике — `Rejected` (запрос не запоминается). Экран обязан слать `Show`
    /// ПОСЛЕ установки своего источника (`UiOverlayCommand::Install`); `Install` и любой уход источника отзывают запрос.
    Scopes(ScopeIntent),
}

/// Чем закончилась команда.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewfinderCommandOutcome {
    /// Настройка изменена; со следующего кадра видоискатель работает по-новому.
    Applied,
    /// Ровно это уже действовало: ничего не изменилось.
    AlreadyInEffect,
    /// Команда неисполнима (причина в тексте); прежняя настройка осталась.
    Rejected {
        /// Почему.
        reason: String,
    },
    /// Бэкенд не показывает камеру (тестовый паттерн, fake): видоискателя нет.
    Unsupported,
}

impl ViewfinderCommand {
    /// Проверка, не требующая состояния бэкенда. Сейчас — только соотношение сторон кадрирования
    /// (намерение о приборах проверено типами `scope-contract`; предел размера из конфига проверяет бэкенд):
    /// зум уже проверен типом `ZoomFactor`, а влезет ли кадрирование в полезную область, станет
    /// известно только на кадре (тогда кадр не обрабатывается, причина — в диагностике).
    pub fn rejection_reason(&self) -> Option<String> {
        match self {
            Self::SetCrop(crop) => crop_rejection_reason(crop),
            Self::SetFraming(ViewfinderFraming { crop, .. }) => {
                crop_aspect_rejection_reason(crop.aspect()).or_else(|| crop.rejection_reason())
            }
            _ => None,
        }
    }
}

/// Почему кадрирование неисполнимо (`None` — исполнимо).
fn crop_rejection_reason(crop: &ViewfinderCrop) -> Option<String> {
    crop_aspect_rejection_reason(crop.aspect)
}

/// Почему соотношение сторон неисполнимо (`None` — исполнимо). Общее правило для фиксированного кадрирования и
/// кадрирования по зуму.
fn crop_aspect_rejection_reason(aspect: CropAspect) -> Option<String> {
    match aspect {
        CropAspect::LongToShort { long, short } if short == 0 || long < short => {
            Some(format!("соотношение сторон {long}:{short}: нужно long ≥ short > 0"))
        }
        _ => None,
    }
}

/// Последняя команда и её итог.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewfinderCommandRecord {
    /// Что просили.
    pub command: ViewfinderCommand,
    /// Что вышло.
    pub outcome: ViewfinderCommandOutcome,
}

/// Учёт команд видоискателя в статусе потока: по итогам и последняя команда.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ViewfinderCommandLog {
    /// Сколько команд применено.
    pub applied: u64,
    /// Сколько команд ничего не изменили (уже действовали).
    pub already_in_effect: u64,
    /// Сколько команд отвергнуто.
    pub rejected: u64,
    /// Сколько команд пришло, когда видоискателя нет (или бэкенд неисправен).
    pub unsupported: u64,
    /// Последняя команда (`None` — команд не было).
    pub last: Option<ViewfinderCommandRecord>,
}

impl ViewfinderCommandLog {
    /// Записывает итог команды.
    pub fn note(&mut self, command: ViewfinderCommand, outcome: ViewfinderCommandOutcome) {
        match &outcome {
            ViewfinderCommandOutcome::Applied => self.applied += 1,
            ViewfinderCommandOutcome::AlreadyInEffect => self.already_in_effect += 1,
            ViewfinderCommandOutcome::Rejected { .. } => self.rejected += 1,
            ViewfinderCommandOutcome::Unsupported => self.unsupported += 1,
        }
        self.last = Some(ViewfinderCommandRecord { command, outcome });
    }
}

#[cfg(test)]
mod tests {
    use super::super::framing::FramingDestination;
    use camera_contract::viewfinder_crop::{ViewfinderCrop, ZoomFactor};
    use isp_vulkan::viewfinder::IspStage;

    use super::*;

    #[test]
    fn broken_aspect_is_rejected_before_reaching_the_backend() {
        let broken = ViewfinderCommand::SetCrop(ViewfinderCrop {
            aspect: CropAspect::LongToShort { long: 3, short: 4 },
            zoom: ZoomFactor::NONE,
            center: camera_contract::viewfinder_crop::CropCenter::ValidAreaCenter,
        });
        assert!(broken.rejection_reason().is_some_and(|reason| reason.contains("3:4")));
        let zero = ViewfinderCommand::SetCrop(ViewfinderCrop {
            aspect: CropAspect::LongToShort { long: 4, short: 0 },
            zoom: ZoomFactor::NONE,
            center: camera_contract::viewfinder_crop::CropCenter::ValidAreaCenter,
        });
        assert!(zero.rejection_reason().is_some());
        let broken_framing = ViewfinderCommand::SetFraming(ViewfinderFraming {
            crop: ViewfinderCrop {
                aspect: CropAspect::LongToShort { long: 3, short: 4 },
                zoom: ZoomFactor::NONE,
                center: camera_contract::viewfinder_crop::CropCenter::ValidAreaCenter,
            }
            .into(),
            destination: FramingDestination::Configured,
        });
        assert!(
            broken_framing
                .rejection_reason()
                .is_some_and(|reason| reason.contains("3:4")),
            "кадрирование внутри SetFraming проверяется так же, как в SetCrop"
        );
        for fine in [
            ViewfinderCommand::SetFraming(ViewfinderFraming {
                crop: ViewfinderCrop::FULL_SENSOR.into(),
                destination: FramingDestination::Configured,
            }),
            ViewfinderCommand::SetCrop(ViewfinderCrop::FULL_SENSOR),
            ViewfinderCommand::SetCrop(ViewfinderCrop {
                aspect: CropAspect::SQUARE,
                zoom: ZoomFactor::NONE,
                center: camera_contract::viewfinder_crop::CropCenter::ValidAreaCenter,
            }),
            ViewfinderCommand::SetPainter(ViewfinderPainter::DebugMosaic),
            ViewfinderCommand::SetIspStages(IspStageSet::ALL.without(IspStage::Tone)),
            ViewfinderCommand::SetHeadroomBench(HeadroomBench::Enabled),
        ] {
            assert_eq!(fine.rejection_reason(), None, "{fine:?}");
        }
    }

    #[test]
    fn log_counts_each_outcome_and_keeps_the_last_command() {
        let mut log = ViewfinderCommandLog::default();
        let painter = ViewfinderCommand::SetPainter(ViewfinderPainter::DebugMosaic);
        log.note(painter.clone(), ViewfinderCommandOutcome::Applied);
        log.note(painter.clone(), ViewfinderCommandOutcome::AlreadyInEffect);
        log.note(painter, ViewfinderCommandOutcome::Unsupported);
        let stages = ViewfinderCommand::SetIspStages(IspStageSet::ALL);
        let rejected = ViewfinderCommandOutcome::Rejected {
            reason: "причина".to_owned(),
        };
        log.note(stages.clone(), rejected.clone());
        assert_eq!(
            (log.applied, log.already_in_effect, log.unsupported, log.rejected),
            (1, 1, 1, 1)
        );
        assert_eq!(
            log.last,
            Some(ViewfinderCommandRecord {
                command: stages,
                outcome: rejected
            })
        );
    }
}
