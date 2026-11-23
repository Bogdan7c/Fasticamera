//! Тесты исполнителя намерений по приборам: команда рендеру равна намерению, исходы доставки считаются по видам,
//! ничего лишнего рендеру не уходит. Порт рендера подставной.

use render_loop::ui_overlay::UiOverlayCommand;
use scope_contract::{ScopeImageSize, ScopeKind, ScopeRequest, VectorscopeAxes};

use super::*;
use crate::gpu_test_support::RecordingRenderControl;

/// Порт рендера с заданным итогом доставки.
struct FixedDelivery(fn() -> ControlDelivery);

impl RenderControl for FixedDelivery {
    fn adjust(&self, _adjustment: RenderAdjustment) -> ControlDelivery {
        (self.0)()
    }

    fn ui_overlay(&self, _command: UiOverlayCommand) -> ControlDelivery {
        (self.0)()
    }
}

fn show_request() -> ScopeIntent {
    ScopeIntent::Show(ScopeRequest {
        kind: ScopeKind::Vectorscope,
        vectorscope_axes: VectorscopeAxes::OklabAb,
        scan: scope_contract::ScopeScan::Upright,
        image_size: ScopeImageSize::try_new(200, 100).expect("размер в пределах"),
    })
}

fn sent_scope_intents(render: &RecordingRenderControl) -> Vec<ScopeIntent> {
    render
        .recorded()
        .into_iter()
        .map(|adjustment| match adjustment {
            RenderAdjustment::Viewfinder(ViewfinderCommand::Scopes(intent)) => intent,
            other => panic!("рендеру ушло не намерение по прибору: {other:?}"),
        })
        .collect()
}

#[test]
fn show_and_hide_reach_the_render_unchanged_and_in_order() {
    let mut effects = ScopeEffects::default();
    let render = RecordingRenderControl::new();

    effects.run_intent(show_request(), render.as_ref());
    effects.run_intent(ScopeIntent::Hide, render.as_ref());

    assert_eq!(sent_scope_intents(&render), vec![show_request(), ScopeIntent::Hide]);
    assert_eq!(
        effects.counters(),
        ScopeCounters {
            show_delivered: 1,
            hide_delivered: 1,
            ..ScopeCounters::default()
        }
    );
}

#[test]
fn a_gone_render_is_counted_as_gone_and_nothing_else() {
    let mut effects = ScopeEffects::default();

    effects.run_intent(show_request(), &FixedDelivery(|| ControlDelivery::RenderGone));

    assert_eq!(
        effects.counters(),
        ScopeCounters {
            render_gone: 1,
            ..ScopeCounters::default()
        }
    );
}

#[test]
fn an_unavailable_render_is_counted_as_unavailable_for_hide_too() {
    let mut effects = ScopeEffects::default();

    effects.run_intent(
        ScopeIntent::Hide,
        &FixedDelivery(|| ControlDelivery::RenderUnavailable {
            reason: "нет GPU".to_owned(),
        }),
    );

    assert_eq!(
        effects.counters(),
        ScopeCounters {
            render_unavailable: 1,
            ..ScopeCounters::default()
        }
    );
}
