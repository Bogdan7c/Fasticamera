//! Автоматика 3A внутри движка (сессия 08): тап и блокировка, ручные параметры, серии,
//! указание баланса белого.
//!
//! Движок остаётся уровневым: команды меняют **намерение** (`Desired::three_a`), а
//! `reconcile` сводит его с реальным запросом HAL. Исполнители (`ExposureController`,
//! `FocusController`) решают, *как* выполнить намерение на этой камере; движок только:
//! * чисто объединяет их планы в `RequestControls` ([`planning`]);
//! * перестраивает повторяющийся запрос, если план изменился, и отправляет разовый триггер;
//! * передаёт им результаты кадров, ведёт таймер тапа ([`results`]);
//! * ведёт серии с брекетингом ([`bracket`]).
//!
//! Белая сюда не входит: её владелец — `engine/white`, и он живёт вне `Desired`.
//!
//! Раскладка файлов: `decide` — итоги команд, `planning` — план запроса, `results` — результаты
//! и таймеры, `bracket` — серии.

mod bracket;
mod decide;
mod exposure_carry;
mod exposure_directive;
mod flash_access;
mod planning;
mod results;

use std::time::Instant;

use crate::flash_status::FlashStatus;
use camera_contract::auto_exposure::ExposureDirective;
use camera_contract::three_a::{DirectiveSlot, PointRequestId};
pub(in crate::engine) use planning::PlannedRequest;

use crate::three_a_control::{ExposureController, FocusController, ThreeAIntent};
use crate::three_a_status::{AutoExposureProgress, ControlAdjustment, LastThreeAResult, ThreeACounters, ThreeAStatus};
use crate::white::WhiteStatus;
use bracket::BracketRun;
use exposure_directive::{ExposurePublisher, refine_progress};

/// Всё состояние 3A, которым владеет движок, кроме самого намерения (оно в `Desired`).
pub(super) struct ThreeAEngine {
    exposure: Box<dyn ExposureController>,
    focus: Box<dyn FocusController>,
    exposure_directive: ExposurePublisher,
    /// Камера, на которой стартовал последний поток: в её произведении записана память исполнителя
    /// экспозиции (см. `exposure_carry`).
    exposure_camera: Option<camera_contract::ids::CameraId>,
    /// Поправки ручных значений, как их видит снимок.
    adjustments: Vec<ControlAdjustment>,
    /// Что уже объявлено событием `ControlsAdjusted` (чтобы не повторять то же самое).
    announced_adjustments: Vec<ControlAdjustment>,
    last_result: Option<LastThreeAResult>,
    counters: ThreeACounters,
    /// До какого срока ждём итога этого тапа.
    focus_settle_deadline: Option<(PointRequestId, Instant)>,
    /// Серия, ожидающая отправки или результатов (одна за раз).
    bracket: Option<BracketRun>,
}

impl ThreeAEngine {
    pub(super) fn new(
        exposure: Box<dyn ExposureController>,
        focus: Box<dyn FocusController>,
        exposure_directives: DirectiveSlot<ExposureDirective>,
        initial_intent: &ThreeAIntent,
    ) -> Self {
        let mut exposure_directive = ExposurePublisher::new(exposure_directives);
        exposure_directive.publish_if_changed(initial_intent, None);
        Self {
            exposure,
            focus,
            exposure_directive,
            exposure_camera: None,
            adjustments: Vec::new(),
            announced_adjustments: Vec::new(),
            last_result: None,
            counters: ThreeACounters::default(),
            focus_settle_deadline: None,
            bracket: None,
        }
    }

    /// Снимок для [`crate::CameraStatus`]: намерение берётся из желаемого состояния.
    pub(super) fn status(&self, intent: &ThreeAIntent) -> ThreeAStatus {
        ThreeAStatus {
            point: intent.point,
            lock: intent.lock,
            profile: intent.profile,
            focus: self.focus.progress(),
            auto_exposure: self.auto_exposure_progress(intent),
            controls: intent.controls,
            adjustments: self.adjustments.clone(),
            last_result: self.last_result,
            counters: ThreeACounters {
                // Счётчик ведёт исполнитель экспозиции (он единственный знает про чужие предложения).
                ae_foreign_camera_proposals_rejected: self.exposure.foreign_camera_proposals_rejected(),
                ..self.counters
            },
            // Вспышку собирает `CameraEngine::status`: статусу нужны возможности камеры и поток.
            flash: FlashStatus::default(),
            // Белую собирает `CameraEngine::status`: её владелец — не 3A.
            white: WhiteStatus::initial(),
        }
    }

    /// Ближайший срок таймеров 3A (итог тапа, результаты серии).
    pub(super) fn next_deadline(&self) -> Option<Instant> {
        let focus = self.focus_settle_deadline.map(|(_, deadline)| deadline);
        let bracket = self.bracket.as_ref().and_then(BracketRun::deadline);
        [focus, bracket].into_iter().flatten().min()
    }

    /// Состояние нашего AE для снимка (см. [`refine_progress`]).
    fn auto_exposure_progress(&self, intent: &ThreeAIntent) -> Option<AutoExposureProgress> {
        refine_progress(
            self.exposure.progress(),
            &intent.controls,
            intent.lock,
            &self.adjustments,
        )
    }

    /// Намерение могло измениться командой без пересборки запроса — донести его до рендера.
    pub(super) fn publish_exposure_intent(&mut self, intent: &ThreeAIntent) {
        self.exposure_directive.publish_intent_keeping_applied(intent);
    }
}
