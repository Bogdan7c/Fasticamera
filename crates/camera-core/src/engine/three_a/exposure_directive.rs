//! Указание нашему AE на потоке рендера (решение 08a, п. 3): где мерить, блокировка, кто
//! ведёт экспозицию, компенсация и что реально стоит в повторяющемся запросе.
//!
//! Как и AWB ([`super::directive`]), указание кладётся в [`DirectiveSlot`] **только при
//! изменении**: поколение слота растёт на настоящих сменах, читатель не перечитывает то же.
//! Поле `applied` публикуется после того, как повторяющийся запрос реально поставлен: по нему
//! рендер считает остаточную цифровую подтяжку, пока сенсор догоняет новую экспозицию.

use camera_contract::auto_exposure::{
    AppliedExposure, CarriedSceneExposure, ExposureAuthority, ExposureDirective, ExposureProduct, ProductReach,
};
use camera_contract::control::{Control, ManualControl};
use camera_contract::three_a::{AeAfLock, DirectiveSlot};

use crate::request_controls::ExposurePlan;
use crate::three_a_control::ThreeAIntent;
use crate::three_a_status::{AdjustmentReason, AutoExposureProgress, ControlAdjustment, ControlParameter};

/// Писатель ящика указаний AE, помнящий последнее опубликованное.
pub(super) struct ExposurePublisher {
    slot: DirectiveSlot<ExposureDirective>,
    last_published: Option<ExposureDirective>,
}

impl ExposurePublisher {
    pub(super) fn new(slot: DirectiveSlot<ExposureDirective>) -> Self {
        Self {
            slot,
            last_published: None,
        }
    }

    /// Опубликовать указание для намерения и применённой экспозиции, если оно изменилось.
    pub(super) fn publish_if_changed(&mut self, intent: &ThreeAIntent, applied: Option<AppliedExposure>) {
        // Ожидаемая экспозиция потока живёт дольше намерения и запроса: меняется только на границе потока.
        let stream_start = self.last_published.and_then(|last| last.stream_start);
        self.publish_directive_if_changed(ExposureDirective {
            stream_start,
            ..directive_for(intent, applied)
        });
    }

    /// Сообщить рендеру, какую экспозицию ожидать от НОВОГО потока до его первого замера (`None` — ожидать
    /// нечего). Вызывается до смены потока, чтобы указание было на месте к первому кадру нового потока.
    pub(super) fn announce_stream_start(&mut self, stream_start: Option<CarriedSceneExposure>) {
        let last = self.last_published.unwrap_or(ExposureDirective::DEFAULT);
        self.publish_directive_if_changed(ExposureDirective { stream_start, ..last });
    }

    /// Намерение изменилось (например, `Close` сбросил его), а запрос не пересобирался:
    /// обновить всё, кроме `applied`, которое остаётся тем, что реально стоит в запросе.
    pub(super) fn publish_intent_keeping_applied(&mut self, intent: &ThreeAIntent) {
        let applied = self.last_published.and_then(|last| last.applied);
        self.publish_if_changed(intent, applied);
    }

    /// Повторяющегося запроса больше нет: убрать `applied`, остальное оставить как было.
    ///
    /// `stream_start` НЕ трогаем сознательно: при обмене тёплой он объявлен ДО обмена, а конец потока
    /// наступает уже после; сброс оставил бы рендеру окно «ожидание пропало». Устареть оно не может: каждый
    /// новый поток объявляет своё ожидание (или «ожидать нечего») при старте, а рендер применяет его только к
    /// камере, названной в нём, и только пока у потока нет собственного замера.
    pub(super) fn withdraw_applied(&mut self) {
        if let Some(last) = self.last_published {
            self.publish_directive_if_changed(ExposureDirective { applied: None, ..last });
        }
    }

    fn publish_directive_if_changed(&mut self, directive: ExposureDirective) {
        if self.last_published == Some(directive) {
            return;
        }
        self.slot.publish(directive);
        self.last_published = Some(directive);
    }
}

/// Чистая функция: намерение и применённая экспозиция → указание.
pub(super) fn directive_for(intent: &ThreeAIntent, applied: Option<AppliedExposure>) -> ExposureDirective {
    ExposureDirective {
        metering: intent.point,
        lock: intent.lock,
        profile: intent.profile,
        authority: authority_of(&intent.controls),
        bias: intent.controls.exposure_bias,
        applied,
        // Ожидание нового потока — не часть намерения: его дописывает издатель (`publish_if_changed`).
        stream_start: None,
    }
}

fn authority_of(controls: &ManualControl) -> ExposureAuthority {
    match (controls.iso, controls.shutter) {
        (Control::Auto, Control::Auto) => ExposureAuthority::Automatic,
        (Control::Manual(_), Control::Manual(_)) => ExposureAuthority::Manual,
        (Control::Auto, Control::Manual(_)) | (Control::Manual(_), Control::Auto) => ExposureAuthority::SemiAutomatic,
    }
}

/// Что стоит в запросе: произведение «выдержка × ISO», честная нехватка пределов и достижимый отрезок.
/// `None` для вендорского авто (произведение не известно движку).
pub(super) fn applied_from_plan(
    plan: &ExposurePlan,
    adjustments: &[ControlAdjustment],
    reach: ProductReach,
) -> Option<AppliedExposure> {
    let ExposurePlan::Sensor { exposure_time, iso } = plan else {
        return None;
    };
    let shortfall_ev = adjustments
        .iter()
        .find(|adjustment| {
            adjustment.parameter == ControlParameter::ExposureTarget
                && adjustment.reason == AdjustmentReason::TargetNotReached
        })
        .map_or(0.0, |adjustment| (adjustment.applied - adjustment.requested) as f32);
    Some(AppliedExposure {
        product: ExposureProduct::from_sensor(*exposure_time, *iso),
        shortfall_ev,
        reach,
    })
}

/// Состояние нашего AE для снимка: к ответу исполнителя движок добавляет то, что знает сам —
/// «обе величины ручные» и «упёрлись в предел» (из поправок последнего плана).
/// `None`, если исполнитель сказать про свой замер нечего (вендорский AE).
pub(super) fn refine_progress(
    measured: Option<AutoExposureProgress>,
    controls: &ManualControl,
    lock: AeAfLock,
    adjustments: &[ControlAdjustment],
) -> Option<AutoExposureProgress> {
    let measured = measured?;
    if controls.exposure_fully_manual() {
        return Some(AutoExposureProgress::Manual);
    }
    // Блокировка важнее предела: цель заморожена, пользователю нужно «держу», а не «упёрлись».
    if lock == AeAfLock::Locked && measured == AutoExposureProgress::Holding {
        return Some(measured);
    }
    let shortfall = adjustments.iter().find(|adjustment| {
        adjustment.parameter == ControlParameter::ExposureTarget
            && adjustment.reason == AdjustmentReason::TargetNotReached
    });
    Some(match shortfall {
        Some(adjustment) => AutoExposureProgress::LimitReached {
            shortfall_ev: (adjustment.applied - adjustment.requested) as f32,
        },
        None => measured,
    })
}
