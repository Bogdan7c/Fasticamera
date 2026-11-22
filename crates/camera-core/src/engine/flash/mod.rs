//! Снимок со вспышкой внутри движка (сессия 14d, решения 63, 153, 158, 159).
//!
//! Владелец состояния — [`FlashEngine`] (поле `CameraEngine::flash`): текущая последовательность,
//! счётчики, поколение прочитанных замеров и необязательный наблюдатель итогов. Остальной движок
//! знает о вспышке только по маленьким точкам входа ниже, а не по устройству последовательности.
//!
//! Последовательность (фазы — [`FlashPhase`]):
//! 1. `decide` — команда принимается или отказывается с причиной (и командой, и итогом снимка).
//! 2. `preflash` — фонарик в ПОВТОРЯЮЩЕМСЯ запросе со своей экспозицией; рендер мерит кадры и
//!    кладёт линейные замеры в ящик нашего AE; движок их читает по сигналу пробуждения
//!    (`refresh_three_a`), пересчитывает пересвеченный замер и считает кадр (`plan`).
//! 3. `sequence` — пауза без фонарика, разовый запрос со вспышкой, ожидание результата, сроки,
//!    единственный итог.
//! 4. `advice` — совет для Auto и статус.
//!
//! Инварианты (проверяются тестами на каждом пути выхода):
//! * фонарик стоит в повторяющемся запросе тогда и только тогда, когда фаза — `Preflash`; его
//!   добавляет единственное место — `apply_flash_to_plan` (`sequence`), через общий путь планирования,
//!   поэтому триггеры фокуса во время предвспышки тоже несут фонарик, а тёплые камеры — никогда;
//! * указание рендеру «идёт предвспышка» опубликовано тогда и только тогда, когда фаза — `Preflash`;
//! * на каждую команду, дошедшую до движка, — ровно один итог (событие и наблюдатель);
//! * пока последовательность не `Idle`, наш AE не перестраивает запрос: его решения догоняются
//!   одним шагом при возврате в `Idle`.

mod advice;
mod decide;
mod plan;
mod preflash;
mod sequence;

use std::sync::Arc;
use std::time::Instant;

use camera_contract::auto_exposure::{CameraTag, ExposureProduct};
use camera_contract::capture_result::SensorTimestamp;
use camera_contract::control::{ExposureTime, Iso};
use camera_contract::flash::{FlashControl, FlashShotId, PreflashSample};
use camera_contract::ids::CameraId;
use camera_contract::three_a::DirectiveGeneration;

use crate::flash_status::{FlashCounters, FlashPhase, FlashStillObserver};
use plan::PlannedShot;

/// Экспозиция кадров предвспышки: то, что реально стоит в запросе с фонариком.
#[derive(Debug, Clone, Copy)]
pub(super) struct PreflashExposure {
    pub(super) exposure_time: ExposureTime,
    pub(super) iso: Iso,
    /// Произведение «выдержка × ISO» уже после округления ISO: с ним сравниваются замеры кадров.
    pub(super) product: ExposureProduct,
}

impl PreflashExposure {
    /// Тот же ли это запрос (выдержка и ISO после квантования), что и `other`. Когда шаг перемера упирается
    /// в границы камеры, «уменьшенная» экспозиция совпадает со старой, и перемер ничего бы не изменил.
    pub(super) fn is_same_request_as(&self, other: &PreflashExposure) -> bool {
        self.exposure_time == other.exposure_time && self.iso == other.iso
    }
}

/// Предвспышка: ждём линейные замеры.
#[derive(Debug, Clone, Copy)]
pub(super) struct Preflash {
    /// Сколько раз замер с фонариком был пересвечен и экспозицию уменьшили.
    pub(super) remeasures: u32,
    /// Срок на годный замер: `preflash_timeout_ms` от первого запроса с фонариком. Каждый перемер
    /// ПРОДЛЕВАЕТ его ещё на `preflash_timeout_ms` (после перестройки запроса фонарик заново светит
    /// 7–8 кадров до замера), поэтому худший срок предвспышки — `(1 + max_preflash_remeasures)` таймаутов.
    pub(super) deadline: Instant,
    pub(super) exposure: PreflashExposure,
    /// Замеры с фонариком не новее этой метки не берутся: ящик хранит последний замер каждого света,
    /// и в новом поколении может лежать замер с фонариком ПРОШЛОГО снимка с тем же произведением.
    pub(super) torch_frame_floor: Option<SensorTimestamp>,
    /// Метка последнего результата повторяющегося запроса в момент, когда запрос с фонариком был выдан.
    /// Неизменна на всю предвспышку (в отличие от `torch_frame_floor`, который сдвигает перемер).
    /// Замер без фонарика годится для плана, только если снят на кадре не новее этой метки: кадр,
    /// экспонированный уже после запроса, может быть частично освещён (на части камер первый освещённый
    /// кадр приходит БЕЗ эха фонарика и выглядит «обычным»). `None` — результатов ещё не было,
    /// доказать, что кадр снят до запроса, нечем.
    pub(super) request_floor: Option<SensorTimestamp>,
    /// Лучший замер без фонарика этой камеры, снятый ДО запроса фонарика (см. `request_floor`).
    pub(super) ambient: Option<PreflashSample>,
}

impl Preflash {
    /// Годится ли замер без фонарика как «свет сцены» для плана: камера та же и кадр снят не позже
    /// последнего результата, виденного к моменту запроса фонарика (`request_floor`).
    /// Без метки (`None`) доказать нечем — не годится.
    pub(super) fn is_pre_request_ambient(
        sample: &PreflashSample,
        camera: CameraTag,
        request_floor: Option<SensorTimestamp>,
    ) -> bool {
        sample.measured_on == camera && request_floor.is_some_and(|floor| sample.frame <= floor)
    }
}

/// Где последовательность сейчас.
#[derive(Debug, Clone, Copy)]
pub(super) enum Stage {
    Preflash(Preflash),
    /// Фонарик выключен; разовый запрос уйдёт в `submit_at`.
    Gap {
        planned: PlannedShot,
        submit_at: Instant,
    },
    /// Разовый запрос отправлен; результат ждём до `deadline`.
    Fired {
        planned: PlannedShot,
        deadline: Instant,
    },
}

/// Идущая последовательность снимка со вспышкой.
#[derive(Debug, Clone)]
pub(super) struct FlashRun {
    pub(super) shot: FlashShotId,
    /// Камера, на которой последовательность началась: сменилась она — объектив сменился.
    pub(super) camera: CameraId,
    /// Когда команда принята: от этого считается `elapsed` итога.
    pub(super) accepted_at: Instant,
    pub(super) stage: Stage,
}

/// Всё состояние вспышки движка.
pub(super) struct FlashEngine {
    run: Option<FlashRun>,
    counters: FlashCounters,
    /// Поколение последних прочитанных замеров предвспышки (переживает снимки: ящик хранит последнее).
    samples_seen: Option<DirectiveGeneration>,
    observer: Option<Arc<dyn FlashStillObserver>>,
}

impl FlashEngine {
    pub(super) fn new(observer: Option<Arc<dyn FlashStillObserver>>) -> Self {
        Self {
            run: None,
            counters: FlashCounters::default(),
            samples_seen: None,
            observer,
        }
    }

    /// Идёт ли последовательность в любой фазе.
    pub(super) fn in_progress(&self) -> bool {
        self.run.is_some()
    }

    pub(super) fn phase(&self) -> FlashPhase {
        match self.run.as_ref() {
            None => FlashPhase::Idle,
            Some(FlashRun {
                shot,
                stage: Stage::Preflash(preflash),
                ..
            }) => FlashPhase::Preflash {
                shot: *shot,
                remeasures: preflash.remeasures,
            },
            Some(FlashRun {
                shot,
                stage: Stage::Gap { .. },
                ..
            }) => FlashPhase::Gap { shot: *shot },
            Some(FlashRun {
                shot,
                stage: Stage::Fired { .. },
                ..
            }) => FlashPhase::Fired { shot: *shot },
        }
    }

    /// Экспозиция предвспышки, пока фаза — `Preflash`; иначе `None`. По ней же повторяющийся запрос
    /// получает фонарик: условие «фонарик ⇔ Preflash» выражено одним местом.
    /// Экспозиция и светодиод, которых последовательность требует от ПОВТОРЯЮЩЕГОСЯ запроса: только на время
    /// предвспышки (фонарик + экспозиция предвспышки). В паузе и ожидании результата повторяющийся запрос —
    /// обычный план нашего AE без фонарика.
    pub(super) fn repeating_override(&self) -> Option<(ExposureTime, Iso, FlashControl)> {
        match self.run.as_ref()?.stage {
            Stage::Preflash(preflash) => Some((
                preflash.exposure.exposure_time,
                preflash.exposure.iso,
                FlashControl::Torch,
            )),
            Stage::Gap { .. } | Stage::Fired { .. } => None,
        }
    }

    /// Ближайший срок последовательности.
    pub(super) fn deadline(&self) -> Option<Instant> {
        match self.run.as_ref()?.stage {
            Stage::Preflash(preflash) => Some(preflash.deadline),
            Stage::Gap { submit_at, .. } => Some(submit_at),
            Stage::Fired { deadline, .. } => Some(deadline),
        }
    }
}
