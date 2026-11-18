//! Связка нашего AE: сторона рендера по переключателю и жизнь привязки толчка.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use camera_contract::auto_exposure::{
    ExposureProduct, ExposureProposal, MeasurementQuality, MeteredArea, ProposalConvergence, RefreshSignal,
    SignalDelivery,
};
use camera_contract::capture_result::SensorTimestamp;

use super::*;

/// Получатель толчка, который считает пробуждения.
#[derive(Default)]
struct CountingReceiver(AtomicU64);

impl RefreshSignal for CountingReceiver {
    fn signal(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn proposal() -> ExposureProposal {
    ExposureProposal {
        measured_frame: SensorTimestamp::from_nanos(1),
        target: ExposureProduct::from_seconds_iso(2.0).expect("цель"),
        metered_area: MeteredArea::WholeFrame,
        convergence: ProposalConvergence::Searching,
        quality: MeasurementQuality::Measured,
    }
}

#[test]
fn default_controller_is_ours() {
    assert_eq!(ExposureControllerKind::default(), ExposureControllerKind::Own);
}

#[test]
fn render_end_exists_only_for_our_controller() {
    let link = AutoExposureLink::new();
    assert!(link.render_end(ExposureControllerKind::Vendor).is_none());
    assert!(link.render_end(ExposureControllerKind::Own).is_some());
}

#[test]
fn render_end_shares_the_boxes_with_the_camera_side() {
    let link = AutoExposureLink::new();
    let render = link.render_end(ExposureControllerKind::Own).expect("сторона рендера");

    // Предложение рендера видно со стороны камеры.
    render.proposals.publish(proposal());
    let (_, seen) = link.engine_proposals().latest().expect("предложение в ящике");
    assert_eq!(seen, proposal());

    // Указание движка видно рендеру.
    let directive = camera_contract::auto_exposure::ExposureDirective::DEFAULT;
    link.engine_directive_slot().publish(directive);
    assert_eq!(render.directives.latest().map(|(_, value)| value), Some(directive));
}

#[test]
fn binding_delivers_pushes_and_dropping_it_unbinds() {
    let link = AutoExposureLink::new();
    let render = link.render_end(ExposureControllerKind::Own).expect("сторона рендера");
    let receiver = Arc::new(CountingReceiver::default());

    assert_eq!(
        render.proposals.publish(proposal()).1,
        SignalDelivery::NotBoundYet,
        "до привязки толчок пропущен"
    );

    // Привязка после пропущенного толчка сразу будит получателя один раз: ящик не остаётся непрочитанным.
    let binding = link.bind_engine(receiver.clone());
    assert_eq!(
        receiver.0.load(Ordering::SeqCst),
        1,
        "пропущенный толчок доставлен при привязке"
    );
    assert_eq!(render.proposals.publish(proposal()).1, SignalDelivery::Delivered);
    assert_eq!(
        receiver.0.load(Ordering::SeqCst),
        2,
        "дальше каждое предложение будит получателя"
    );

    drop(binding);
    assert_eq!(
        render.proposals.publish(proposal()).1,
        SignalDelivery::NotBoundYet,
        "после остановки движка получателя нет"
    );
    assert_eq!(receiver.0.load(Ordering::SeqCst), 2, "отвязанный получатель не будится");
}

#[test]
fn our_controller_without_a_renderer_falls_back_to_vendor_with_a_reason() {
    let unavailable = RenderAutoExposure::Unavailable {
        reason: "vsync не запущен".to_owned(),
    };
    let own = ExposureControllerChoice::resolve(ExposureControllerKind::Own, &unavailable);
    assert_eq!(own.configured, ExposureControllerKind::Own);
    assert_eq!(own.effective, ExposureControllerKind::Vendor);
    assert!(
        own.fallback_reason
            .is_some_and(|reason| reason.contains("vsync не запущен"))
    );

    let connected = ExposureControllerChoice::resolve(ExposureControllerKind::Own, &RenderAutoExposure::Connected);
    assert_eq!(
        (connected.effective, connected.fallback_reason),
        (ExposureControllerKind::Own, None)
    );

    // Вендорский AE рендера не ждёт: замены и причины нет.
    let vendor = ExposureControllerChoice::resolve(ExposureControllerKind::Vendor, &unavailable);
    assert_eq!(
        (vendor.effective, vendor.fallback_reason),
        (ExposureControllerKind::Vendor, None)
    );
}
