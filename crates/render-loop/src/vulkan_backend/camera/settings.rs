//! Настройки видоискателя, которые меняются на ходу командами (`ViewfinderCommand`).
//!
//! Владелец — бэкенд показа: настройки переживают пересоздание surface, а живая сессия показа
//! получает копию после каждой применённой команды. Здесь только правило «что меняет команда»
//! (чистая логика, проверяется без GPU).

use std::sync::Arc;

use isp_vulkan::viewfinder::IspStageSet;
use look_contract::{LookSettings, ToneBaseModel};
use scope_contract::{ScopeIntent, ScopeRequest};

use super::ui_state::UiSourcePresence;
use crate::config::{ViewfinderConfig, ViewfinderConfigError, ViewfinderPlacement};
use crate::viewfinder::{
    EffectivePlacement, FramingCrop, FramingDestination, GpuWorkSuppression, HeadroomBench, SuppressibleGpuWork,
    ViewfinderCommand, ViewfinderCommandOutcome, ViewfinderPainter,
};

/// Действующие настройки видоискателя.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewfinderSettings {
    /// Чем рисовать.
    pub painter: ViewfinderPainter,
    /// Включённые стадии ISP.
    pub stages: IspStageSet,
    /// Кадрирование.
    pub crop: FramingCrop,
    /// Положение на экране по конфигу (действует, пока команда не задала прямоугольник).
    pub placement: ViewfinderPlacement,
    /// Место из команды `SetFraming`: `Configured` — как в конфиге, иначе прямоугольник экрана камеры.
    pub destination: FramingDestination,
    /// Стенд «запас под видео» (V4): на старте всегда выключен, включается только командой.
    pub headroom: HeadroomBench,
    /// Вид картинки (решение 178). Стартовое значение задаёт сборка приложения (`with_initial_look`), чтобы оно
    /// действовало с первого кадра; дальше меняется командой `SetLook`.
    pub look: Arc<LookSettings>,
    /// Модель основы тона (решение 195: основа — часть вида). Стартует с `viewfinder.tone.base`, приложение
    /// заменяет её основой активного рецепта (`with_initial_tone_base`), дальше меняется командой `SetToneBase`.
    pub tone_base: ToneBaseModel,
    /// Какой цветовой прибор просит экран (14h). `None` — приборы закрыты: проход не записывается, объекты GPU
    /// освобождены. Единственный владелец запроса; цепочка показа получает его копией вместе с остальными настройками.
    /// Запрос действует, пока установлен источник UI: уход источника отзывает его ([`Self::withdraw_scopes`]).
    pub scopes: Option<ScopeRequest>,
    /// Наибольшая сторона картинки прибора, которую принимает команда `Scopes` (из конфига, пикселей).
    pub scope_picture_side_limit_px: u32,
    /// Какие работы GPU временно не выполняются ради A/B-замера тока (16a). Продукт — `NONE`; меняется только командой
    /// `SuppressGpuWork`. Маска НЕ трогает запрос приборов (`scopes`), рисовальщик, стадии и установленный источник UI:
    /// после её снятия работа возобновляется сама.
    pub gpu_work_suppression: GpuWorkSuppression,
}

/// Причина отказа `Scopes(Show)` без источника UI.
pub const SCOPE_SHOW_WITHOUT_UI_SOURCE: &str =
    "приборы нужны только для UI: источник кадра UI не установлен (сначала Install, потом Show)";

/// Итог отзыва запроса приборов самим рендером ([`ViewfinderSettings::withdraw_scopes`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeWithdrawal {
    /// Запрос был и снят: цепочке показа нужны свежие настройки.
    Withdrawn,
    /// Приборы и так были закрыты: ничего не изменилось.
    NothingToWithdraw,
}

impl ViewfinderSettings {
    /// Стартовые настройки из конфига: все стадии ISP включены (продукт), стенд V4 выключен.
    pub fn from_config(config: &ViewfinderConfig) -> Result<Self, ViewfinderConfigError> {
        Ok(Self {
            painter: config.painter,
            stages: IspStageSet::ALL,
            crop: FramingCrop::Fixed(config.crop.to_crop()?),
            placement: config.placement,
            destination: FramingDestination::Configured,
            headroom: HeadroomBench::Disabled,
            look: Arc::new(LookSettings::NEUTRAL),
            tone_base: config.tone.base.into(),
            scopes: None,
            scope_picture_side_limit_px: config.scopes.max_picture_side_px,
            gpu_work_suppression: GpuWorkSuppression::NONE,
        })
    }

    /// Отзывает запрос приборов, не дожидаясь команды экрана: источник UI, которому показывали картинку прибора,
    /// ушёл (снят, заменён новым, сломался), а новый экран начинает без приборов. Владелец запроса — эти настройки,
    /// поэтому снимает его он, а не соседний модуль через поле.
    pub fn withdraw_scopes(&mut self) -> ScopeWithdrawal {
        match self.scopes.take() {
            Some(_) => ScopeWithdrawal::Withdrawn,
            None => ScopeWithdrawal::NothingToWithdraw,
        }
    }

    /// Почему намерение о приборах неисполнимо при этих настройках (`None` — исполнимо). Закрыть приборы можно всегда.
    pub fn scope_rejection_reason(&self, intent: &ScopeIntent) -> Option<String> {
        let ScopeIntent::Show(request) = intent else {
            return None;
        };
        let (width, height) = (request.image_size.width(), request.image_size.height());
        let limit = self.scope_picture_side_limit_px;
        (width > limit || height > limit)
            .then(|| format!("картинка прибора {width}×{height}: сторона больше предела {limit} (viewfinder.scopes)"))
    }

    /// Какой прибор реально считать на кадре: запрос экрана, но только пока приборы кому-то показывать — рисовальщик
    /// ISP (отладочная мозаика приборов не знает) И установленный источник UI. Это ЕДИНСТВЕННОЕ место решения
    /// «считать ли приборы»: тот же источник UI решает, рисуется ли картинка прибора, поэтому счёт и показ не
    /// расходятся (инвариант «закрытые приборы ничего не стоят», решение 190).
    ///
    /// Подавление замером (`ColorScopes`, 16a) тоже решается ЗДЕСЬ: приборы не считаются, но запомненный запрос экрана
    /// (`self.scopes`) не стирается — после снятия подавления этот же метод вернёт его снова без новой команды экрана.
    /// Подавленный UI (`UiOverlayDrawing`) на счёт приборов не влияет: источник остаётся установленным.
    pub fn effective_scope_demand(&self, ui_source: UiSourcePresence) -> Option<ScopeRequest> {
        if self.gpu_work_suppression.suppresses(SuppressibleGpuWork::ColorScopes) {
            return None;
        }
        match (self.painter, ui_source) {
            (ViewfinderPainter::Isp, UiSourcePresence::Installed) => self.scopes,
            _ => None,
        }
    }

    /// Как [`Self::apply`], но с учётом источника UI: `Scopes(Show)` без источника — `Rejected` (считать приборы
    /// некому показывать). Экран обязан слать `Show` ПОСЛЕ установки своего источника (см. `ViewfinderCommand::Scopes`).
    /// `Hide` принимается всегда.
    pub fn apply_with_ui_source(
        &mut self,
        command: ViewfinderCommand,
        ui_source: UiSourcePresence,
    ) -> ViewfinderCommandOutcome {
        if matches!(command, ViewfinderCommand::Scopes(ScopeIntent::Show(_))) && ui_source == UiSourcePresence::Absent {
            return ViewfinderCommandOutcome::Rejected {
                reason: SCOPE_SHOW_WITHOUT_UI_SOURCE.to_owned(),
            };
        }
        self.apply(command)
    }

    /// Действующее место видоискателя: прямоугольник из команды или правило конфига. Единственный
    /// источник истины и для кадра, и для диагностики.
    pub fn effective_placement(&self) -> EffectivePlacement {
        EffectivePlacement::resolve(self.placement, self.destination)
    }

    /// Применяет команду. Отвергнутая команда настроек не меняет.
    pub fn apply(&mut self, command: ViewfinderCommand) -> ViewfinderCommandOutcome {
        if let Some(reason) = command.rejection_reason() {
            return ViewfinderCommandOutcome::Rejected { reason };
        }
        if let ViewfinderCommand::Scopes(intent) = &command
            && let Some(reason) = self.scope_rejection_reason(intent)
        {
            return ViewfinderCommandOutcome::Rejected { reason };
        }
        let before = self.clone();
        match command {
            ViewfinderCommand::SetPainter(painter) => self.painter = painter,
            ViewfinderCommand::SetIspStages(stages) => self.stages = stages,
            ViewfinderCommand::SetCrop(crop) => self.crop = FramingCrop::Fixed(crop),
            // Одной транзакцией: кадрирование и место меняются в одном присваивании, промежуточного
            // состояния «новое кадрирование в старом месте» не существует.
            ViewfinderCommand::SetFraming(framing) => {
                self.crop = framing.crop;
                self.destination = framing.destination;
            }
            ViewfinderCommand::SetHeadroomBench(mode) => self.headroom = mode,
            ViewfinderCommand::SetLook(look) => self.look = look,
            ViewfinderCommand::SetToneBase(model) => self.tone_base = model,
            ViewfinderCommand::Scopes(ScopeIntent::Show(request)) => self.scopes = Some(request),
            ViewfinderCommand::Scopes(ScopeIntent::Hide) => self.scopes = None,
            ViewfinderCommand::SuppressGpuWork(suppression) => self.gpu_work_suppression = suppression,
        }
        if *self == before {
            ViewfinderCommandOutcome::AlreadyInEffect
        } else {
            ViewfinderCommandOutcome::Applied
        }
    }
}

#[cfg(test)]
mod suppression_tests;

#[cfg(test)]
mod tests {
    use camera_contract::viewfinder_crop::{CropAspect, ViewfinderCrop, ZoomFactor};
    use isp_vulkan::viewfinder::IspStage;

    use super::*;
    use crate::viewfinder::ViewfinderFraming;

    fn defaults() -> ViewfinderSettings {
        ViewfinderSettings::from_config(&ViewfinderConfig::default()).expect("конфиг по умолчанию корректен")
    }

    #[test]
    fn defaults_are_the_product_isp_with_all_stages() {
        let settings = defaults();
        assert_eq!(settings.painter, ViewfinderPainter::Isp);
        assert_eq!(settings.stages, IspStageSet::ALL);
        assert_eq!(settings.crop, ViewfinderCrop::FULL_SENSOR.into());
    }

    #[test]
    fn headroom_bench_starts_disabled_and_toggles_without_touching_the_viewfinder() {
        let mut settings = defaults();
        assert_eq!(
            settings.headroom,
            HeadroomBench::Disabled,
            "стенд не продукт: по умолчанию выключен"
        );
        let enable = ViewfinderCommand::SetHeadroomBench(HeadroomBench::Enabled);
        assert_eq!(settings.apply(enable.clone()), ViewfinderCommandOutcome::Applied);
        assert_eq!(settings.apply(enable), ViewfinderCommandOutcome::AlreadyInEffect);
        assert_eq!(settings.headroom, HeadroomBench::Enabled);
        assert_eq!(
            (settings.painter, settings.stages, settings.crop.clone()),
            (
                ViewfinderPainter::Isp,
                IspStageSet::ALL,
                ViewfinderCrop::FULL_SENSOR.into()
            ),
            "видоискатель работает как прежде"
        );
        assert_eq!(
            settings.apply(ViewfinderCommand::SetHeadroomBench(HeadroomBench::Disabled)),
            ViewfinderCommandOutcome::Applied
        );
        assert_eq!(settings.headroom, HeadroomBench::Disabled);
    }

    #[test]
    fn look_starts_neutral_and_set_look_applies_once_then_is_already_in_effect() {
        let mut settings = defaults();
        assert!(settings.look.is_neutral(), "без команды вид — нейтраль");
        let warm = Arc::new(LookSettings {
            warmth: look_contract::LookKnob::new(0.5).expect("ручка в диапазоне"),
            ..LookSettings::NEUTRAL
        });

        assert_eq!(
            settings.apply(ViewfinderCommand::SetLook(Arc::clone(&warm))),
            ViewfinderCommandOutcome::Applied
        );
        assert_eq!(settings.look, warm);
        // Те же значения в другом `Arc`: повторная команда экрана ничего не меняет.
        let same_values = Arc::new((*warm).clone());
        assert_eq!(
            settings.apply(ViewfinderCommand::SetLook(same_values)),
            ViewfinderCommandOutcome::AlreadyInEffect
        );
        assert_eq!(
            settings.apply(ViewfinderCommand::SetLook(Arc::new(LookSettings::NEUTRAL))),
            ViewfinderCommandOutcome::Applied,
            "возврат к нейтрали — тоже изменение"
        );
        assert!(settings.look.is_neutral());
    }

    #[test]
    fn tone_base_starts_from_config_and_set_tone_base_applies_once_then_is_already_in_effect() {
        let mut settings = defaults();
        assert_eq!(settings.tone_base, ToneBaseModel::Agx, "без команды — основа конфига");
        assert_eq!(
            settings.apply(ViewfinderCommand::SetToneBase(ToneBaseModel::Aces)),
            ViewfinderCommandOutcome::Applied
        );
        assert_eq!(settings.tone_base, ToneBaseModel::Aces);
        assert_eq!(
            settings.apply(ViewfinderCommand::SetToneBase(ToneBaseModel::Aces)),
            ViewfinderCommandOutcome::AlreadyInEffect
        );
    }

    #[test]
    fn set_tone_base_touches_nothing_but_the_tone_base_and_leaves_the_look_alone() {
        let mut settings = defaults();
        settings.apply(ViewfinderCommand::SetLook(Arc::new(LookSettings {
            contrast: look_contract::LookKnob::new(0.5).expect("ручка в диапазоне"),
            ..LookSettings::NEUTRAL
        })));
        let before = settings.clone();
        settings.apply(ViewfinderCommand::SetToneBase(ToneBaseModel::Legacy));
        assert_eq!(
            ViewfinderSettings {
                tone_base: before.tone_base,
                ..settings
            },
            before,
            "вид, рисовальщик, стадии и кадрирование остались прежними"
        );
    }

    #[test]
    fn set_look_touches_nothing_but_the_look() {
        let mut settings = defaults();
        let before = settings.clone();
        settings.apply(ViewfinderCommand::SetLook(Arc::new(LookSettings {
            contrast: look_contract::LookKnob::new(-0.25).expect("ручка в диапазоне"),
            ..LookSettings::NEUTRAL
        })));

        assert_eq!(
            ViewfinderSettings {
                look: before.look.clone(),
                ..settings
            },
            before,
            "рисовальщик, стадии, кадрирование и место остались прежними"
        );
    }

    #[test]
    fn painter_switch_applies_once_and_then_is_already_in_effect() {
        let mut settings = defaults();
        let to_debug = ViewfinderCommand::SetPainter(ViewfinderPainter::DebugMosaic);
        assert_eq!(settings.apply(to_debug.clone()), ViewfinderCommandOutcome::Applied);
        assert_eq!(settings.painter, ViewfinderPainter::DebugMosaic);
        assert_eq!(settings.apply(to_debug), ViewfinderCommandOutcome::AlreadyInEffect);
        assert_eq!(
            settings.stages,
            IspStageSet::ALL,
            "смена рисовальщика стадии не трогает"
        );
    }

    #[test]
    fn stage_set_command_replaces_the_whole_set() {
        let mut settings = defaults();
        let without_tone = IspStageSet::ALL.without(IspStage::Tone);
        assert_eq!(
            settings.apply(ViewfinderCommand::SetIspStages(without_tone)),
            ViewfinderCommandOutcome::Applied
        );
        assert!(!settings.stages.contains(IspStage::Tone));
        assert!(settings.stages.contains(IspStage::Color));
        assert_eq!(
            settings.painter,
            ViewfinderPainter::Isp,
            "стадии не меняют рисовальщика"
        );
    }

    #[test]
    fn invalid_crop_is_rejected_and_changes_nothing() {
        let mut settings = defaults();
        let outcome = settings.apply(ViewfinderCommand::SetCrop(ViewfinderCrop {
            aspect: CropAspect::LongToShort { long: 1, short: 2 },
            zoom: ZoomFactor::NONE,
            center: camera_contract::viewfinder_crop::CropCenter::ValidAreaCenter,
        }));
        assert!(
            matches!(outcome, ViewfinderCommandOutcome::Rejected { .. }),
            "{outcome:?}"
        );
        assert_eq!(settings, defaults());
        let square = ViewfinderCrop {
            aspect: CropAspect::SQUARE,
            zoom: ZoomFactor::NONE,
            center: camera_contract::viewfinder_crop::CropCenter::ValidAreaCenter,
        };
        assert_eq!(
            settings.apply(ViewfinderCommand::SetCrop(square)),
            ViewfinderCommandOutcome::Applied
        );
        assert_eq!(settings.crop, square.into());
    }
    fn square_crop() -> ViewfinderCrop {
        ViewfinderCrop {
            aspect: CropAspect::SQUARE,
            zoom: ZoomFactor::NONE,
            center: camera_contract::viewfinder_crop::CropCenter::ValidAreaCenter,
        }
    }

    fn upper_half() -> crate::viewfinder::NormalizedScreenRect {
        crate::viewfinder::NormalizedScreenRect::new(0.0, 0.0, 1.0, 0.5).expect("прямоугольник")
    }

    #[test]
    fn framing_changes_crop_and_place_together_then_is_already_in_effect() {
        let mut settings = defaults();
        let framing = ViewfinderFraming {
            crop: square_crop().into(),
            destination: FramingDestination::ScreenRect(upper_half()),
        };
        assert_eq!(
            settings.apply(ViewfinderCommand::SetFraming(framing.clone())),
            ViewfinderCommandOutcome::Applied
        );
        assert_eq!(settings.crop, framing.crop);
        assert_eq!(
            settings.effective_placement(),
            EffectivePlacement::ScreenRect(upper_half())
        );
        assert_eq!(
            settings.apply(ViewfinderCommand::SetFraming(framing.clone())),
            ViewfinderCommandOutcome::AlreadyInEffect,
            "повтор ничего не меняет"
        );
        // Смена только места при том же кадрировании — тоже изменение.
        let back_to_config = ViewfinderFraming {
            crop: square_crop().into(),
            destination: FramingDestination::Configured,
        };
        assert_eq!(
            settings.apply(ViewfinderCommand::SetFraming(back_to_config)),
            ViewfinderCommandOutcome::Applied
        );
        assert_eq!(
            settings.effective_placement(),
            EffectivePlacement::Configured(defaults().placement)
        );
    }

    #[test]
    fn framing_with_a_broken_crop_is_rejected_and_leaves_crop_and_place_untouched() {
        let mut settings = defaults();
        let applied = ViewfinderFraming {
            crop: square_crop().into(),
            destination: FramingDestination::ScreenRect(upper_half()),
        };
        settings.apply(ViewfinderCommand::SetFraming(applied));
        let before = settings.clone();
        let broken = ViewfinderFraming {
            crop: ViewfinderCrop {
                aspect: CropAspect::LongToShort { long: 1, short: 2 },
                zoom: ZoomFactor::NONE,
                center: camera_contract::viewfinder_crop::CropCenter::ValidAreaCenter,
            }
            .into(),
            destination: FramingDestination::Configured,
        };
        let outcome = settings.apply(ViewfinderCommand::SetFraming(broken));
        assert!(
            matches!(outcome, ViewfinderCommandOutcome::Rejected { .. }),
            "{outcome:?}"
        );
        assert_eq!(
            settings, before,
            "место не поменялось вместе с отвергнутым кадрированием"
        );
    }

    #[test]
    fn set_crop_keeps_the_place_set_by_framing() {
        let mut settings = defaults();
        settings.apply(ViewfinderCommand::SetFraming(ViewfinderFraming {
            crop: ViewfinderCrop::FULL_SENSOR.into(),
            destination: FramingDestination::ScreenRect(upper_half()),
        }));
        assert_eq!(
            settings.apply(ViewfinderCommand::SetCrop(square_crop())),
            ViewfinderCommandOutcome::Applied
        );
        assert_eq!(
            settings.effective_placement(),
            EffectivePlacement::ScreenRect(upper_half()),
            "SetCrop меняет только кадрирование"
        );
    }

    fn scope_request(kind: scope_contract::ScopeKind, width: u32, height: u32) -> ScopeRequest {
        ScopeRequest {
            kind,
            vectorscope_axes: scope_contract::VectorscopeAxes::YCbCrBt709,
            vectorscope_zoom: scope_contract::VectorscopeZoom::Normal,
            cie_diagram: scope_contract::CieDiagram::default(),
            histogram_mode: scope_contract::HistogramMode::Rgb,
            scan: scope_contract::ScopeScan::Upright,
            image_size: scope_contract::ScopeImageSize::try_new(width, height).expect("размер"),
        }
    }

    #[test]
    fn scopes_show_replaces_the_request_hide_clears_it_and_repeats_are_already_in_effect() {
        let mut settings = defaults();
        assert_eq!(settings.scopes, None, "приборы по умолчанию закрыты");
        assert_eq!(
            settings.apply(ViewfinderCommand::Scopes(ScopeIntent::Hide)),
            ViewfinderCommandOutcome::AlreadyInEffect,
            "скрыть закрытое — ничего"
        );
        let waveform = scope_request(scope_contract::ScopeKind::Waveform, 400, 300);
        let parade = scope_request(scope_contract::ScopeKind::RgbParade, 400, 300);
        assert_eq!(
            settings.apply(ViewfinderCommand::Scopes(ScopeIntent::Show(waveform))),
            ViewfinderCommandOutcome::Applied
        );
        assert_eq!(settings.scopes, Some(waveform));
        assert_eq!(
            settings.apply(ViewfinderCommand::Scopes(ScopeIntent::Show(waveform))),
            ViewfinderCommandOutcome::AlreadyInEffect
        );
        assert_eq!(
            settings.apply(ViewfinderCommand::Scopes(ScopeIntent::Show(parade))),
            ViewfinderCommandOutcome::Applied
        );
        assert_eq!(settings.scopes, Some(parade), "Show заменяет запрос целиком");
        assert_eq!(
            settings.apply(ViewfinderCommand::Scopes(ScopeIntent::Hide)),
            ViewfinderCommandOutcome::Applied
        );
        assert_eq!(settings.scopes, None);
    }

    #[test]
    fn oversized_scope_picture_is_rejected_and_changes_nothing() {
        let mut settings = defaults();
        let limit = settings.scope_picture_side_limit_px;
        let shown = scope_request(scope_contract::ScopeKind::Waveform, 400, 300);
        settings.apply(ViewfinderCommand::Scopes(ScopeIntent::Show(shown)));
        let before = settings.clone();
        for oversized in [
            scope_request(scope_contract::ScopeKind::Waveform, limit + 1, 10),
            scope_request(scope_contract::ScopeKind::Waveform, 10, limit + 1),
        ] {
            let outcome = settings.apply(ViewfinderCommand::Scopes(ScopeIntent::Show(oversized)));
            assert!(
                matches!(&outcome, ViewfinderCommandOutcome::Rejected { reason } if reason.contains(&limit.to_string())),
                "{outcome:?}"
            );
            assert_eq!(settings, before, "отвергнутая команда прежний запрос не трогает");
        }
        let at_limit = scope_request(scope_contract::ScopeKind::Waveform, limit, limit);
        assert_eq!(
            settings.apply(ViewfinderCommand::Scopes(ScopeIntent::Show(at_limit))),
            ViewfinderCommandOutcome::Applied,
            "ровно предел допустим"
        );
    }

    #[test]
    fn scope_commands_touch_nothing_but_the_scope_request() {
        let mut settings = defaults();
        let before = settings.clone();
        settings.apply(ViewfinderCommand::Scopes(ScopeIntent::Show(scope_request(
            scope_contract::ScopeKind::Vectorscope,
            200,
            200,
        ))));
        assert_eq!(
            ViewfinderSettings {
                scopes: None,
                ..settings
            },
            before,
            "вид, рисовальщик, стадии, кадрирование и место остались прежними"
        );
    }

    #[test]
    fn withdrawal_clears_only_a_present_request() {
        let mut settings = defaults();
        assert_eq!(settings.withdraw_scopes(), ScopeWithdrawal::NothingToWithdraw);
        settings.apply(ViewfinderCommand::Scopes(ScopeIntent::Show(scope_request(
            scope_contract::ScopeKind::Waveform,
            100,
            100,
        ))));
        assert_eq!(settings.withdraw_scopes(), ScopeWithdrawal::Withdrawn);
        assert_eq!(settings.scopes, None);
        assert_eq!(
            settings.apply(ViewfinderCommand::Scopes(ScopeIntent::Hide)),
            ViewfinderCommandOutcome::AlreadyInEffect,
            "после отзыва скрывать нечего"
        );
    }
}
