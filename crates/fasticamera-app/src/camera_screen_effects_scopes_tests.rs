//! Приборы в исполнителе эффектов экрана (14h): `Effect::Scopes` доходит до рендера единственной командой и не
//! задевает соседние эффекты (вид, снимок).

use render_loop::viewfinder::ViewfinderCommand;
use scope_contract::{ScopeImageSize, ScopeIntent, ScopeKind, ScopeRequest, VectorscopeAxes};

use super::*;
use crate::scope_effects::ScopeCounters;
use crate::viewfinder_control::RenderAdjustment;

fn waveform() -> ScopeIntent {
    ScopeIntent::Show(ScopeRequest {
        kind: ScopeKind::Waveform,
        vectorscope_axes: VectorscopeAxes::YCbCrBt709,
        vectorscope_zoom: scope_contract::VectorscopeZoom::Normal,
        cie_diagram: scope_contract::CieDiagram::default(),
        histogram_mode: scope_contract::HistogramMode::Rgb,
        scan: scope_contract::ScopeScan::Upright,
        image_size: ScopeImageSize::try_new(64, 32).expect("размер в пределах"),
    })
}

#[test]
fn a_scopes_effect_reaches_the_render_as_exactly_one_scopes_command() {
    let (mut executor, rig) = build_executor(Behavior::default());

    let feedback = executor.execute(vec![Effect::Scopes(waveform()), Effect::Scopes(ScopeIntent::Hide)]);

    assert!(feedback.is_empty(), "приборы экрану ничего не возвращают");
    let sent: Vec<_> = rig
        .render
        .recorded()
        .into_iter()
        .map(|adjustment| match adjustment {
            RenderAdjustment::Viewfinder(ViewfinderCommand::Scopes(intent)) => Some(intent),
            _ => None,
        })
        .collect();
    assert_eq!(
        sent,
        vec![Some(waveform()), Some(ScopeIntent::Hide)],
        "только они, в порядке эффектов"
    );
    assert_eq!(
        executor.counters().scopes,
        ScopeCounters {
            show_delivered: 1,
            hide_delivered: 1,
            ..ScopeCounters::default()
        }
    );
    assert_eq!(executor.counters().look, Default::default(), "вид картинки не задет");
}

#[test]
fn a_missing_render_is_a_counted_failure_visible_in_the_executor_counters() {
    let (mut executor, _rig) = build_executor(Behavior::default());
    executor.targets.render = Arc::new(FixedDeliveryControl(|| ControlDelivery::RenderGone));

    executor.execute(vec![Effect::Scopes(waveform())]);

    let counters = executor.counters().scopes;
    assert_eq!((counters.render_gone, counters.show_delivered), (1, 0));
}
