//! Автоматика 3A внутри движка (сессия 08): тап и блокировка, ручные параметры, серии,
//! указание баланса белого.
//!
//! Движок остаётся уровневым: команды меняют **намерение** (`Desired::three_a`), а
//! `reconcile` сводит его с реальным запросом HAL. Исполнители (`ExposureController`,
//! `FocusController`) решают, *как* выполнить намерение на этой камере; движок только:
//! * чисто объединяет их планы в `RequestControls` ([`planning`]);
//! * перестраивает повторяющийся запрос, если план изменился, и отправляет разовый триггер;
//! * передаёт им результаты кадров, ведёт таймер тапа ([`results`]);
//! * ведёт серии с брекетингом ([`bracket`]);
//! * публикует режим AWB ([`directive`]).
//!
//! Раскладка файлов: `decide` — итоги команд, `planning` — план запроса, `results` — результаты
//! и таймеры, `bracket` — серии, `directive` — баланс белого.

mod bracket;
mod decide;
mod directive;
mod exposure_carry;
mod exposure_directive;
mod planning;
mod results;

use std::time::Instant;

use camera_contract::three_a::PointRequestId;

use crate::three_a_control::{ExposureController, FocusController, ThreeAIntent};
use crate::three_a_status::{AutoExposureProgress, ControlAdjustment, LastThreeAResult, ThreeACounters, ThreeAStatus};
use crate::three_a_wiring::ThreeAWiring;
use bracket::BracketRun;
use directive::WhiteBalancePublisher;
use exposure_directive::{ExposurePublisher, refine_progress};

/// Всё состояние 3A, которым владеет движок, кроме самого намерения (оно в `Desired`).
pub(super) struct ThreeAEngine {
    exposure: Box<dyn ExposureController>,
    focus: Box<dyn FocusController>,
    white_balance: WhiteBalancePublisher,
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
    pub(super) fn new(wiring: ThreeAWiring, initial_intent: &ThreeAIntent) -> Self {
        let mut white_balance = WhiteBalancePublisher::new(wiring.white_balance);
        // Читатель (поток рендера) должен увидеть режим с первого кадра, а не «ничего».
        white_balance.publish_if_changed(initial_intent);
        let mut exposure_directive = ExposurePublisher::new(wiring.exposure_directives);
        exposure_directive.publish_if_changed(initial_intent, None);
        Self {
            exposure: wiring.exposure,
            focus: wiring.focus,
            white_balance,
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
            counters: self.counters,
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

    pub(super) fn publish_white_balance(&mut self, intent: &ThreeAIntent) {
        self.white_balance.publish_if_changed(intent);
    }

    /// Намерение могло измениться командой без пересборки запроса — донести его до рендера.
    pub(super) fn publish_exposure_intent(&mut self, intent: &ThreeAIntent) {
        self.exposure_directive.publish_intent_keeping_applied(intent);
    }
}
