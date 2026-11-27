//! Подписи прибора в редьюсере (решения 326, 327): запоминаются один раз, повтор — «без изменений», остальное состояние
//! раздела и эффекты не затрагиваются.

use scope_contract::{
    LabelPoint, ScopeKind, ScopeLabel, ScopeLabels, ScopeLabelsState, ScopeMark, TargetColor, VectorscopeAxes,
    VectorscopeZoom,
};

use crate::action::ScreenAction;
use crate::outcome::{ActionOutcome, NoChangeReason};
use crate::test_support::Screen;

fn red_state(x: f32, request: scope_contract::ScopeRequest) -> ScopeLabelsState {
    ScopeLabelsState::Published(ScopeLabels {
        request,
        labels: vec![ScopeLabel {
            mark: ScopeMark::VectorscopeTarget(TargetColor::Red),
            at: LabelPoint::try_new(x, 0.2).expect("точка в картинке"),
        }],
    })
}

#[test]
fn labels_are_stored_once_a_repeat_is_no_change_and_nothing_else_moves() {
    let mut screen = Screen::ready();
    let request = scope_contract::ScopeRequest {
        kind: ScopeKind::Vectorscope,
        vectorscope_axes: VectorscopeAxes::OklabAb,
        vectorscope_zoom: VectorscopeZoom::Double,
        cie_diagram: scope_contract::CieDiagram::default(),
        histogram_mode: scope_contract::HistogramMode::default(),
        scan: scope_contract::ScopeScan::Upright,
        image_size: scope_contract::ScopeImageSize::try_new(300, 300).expect("размер"),
    };
    let calibration_before = screen.state.calibration().clone();

    let first = screen.step(ScreenAction::ScopeLabelsChanged(red_state(0.4, request)));
    assert_eq!(first.outcome, ActionOutcome::Applied);
    assert!(first.effects.is_empty(), "подписи — только рисунок, эффектов нет");

    // Выбор раздела (прибор, оси, зум, запрос рендеру) подписи не трогают.
    let calibration = screen.state.calibration();
    assert_eq!(calibration.scope_kind(), calibration_before.scope_kind());
    assert_eq!(calibration.vectorscope_axes(), calibration_before.vectorscope_axes());
    assert_eq!(calibration.vectorscope_zoom(), calibration_before.vectorscope_zoom());
    assert_eq!(calibration.announced_scope(), calibration_before.announced_scope());

    let repeat = screen.step(ScreenAction::ScopeLabelsChanged(red_state(0.4, request)));
    assert_eq!(repeat.outcome, ActionOutcome::NoChange(NoChangeReason::ValueUnchanged));

    let moved = screen.step(ScreenAction::ScopeLabelsChanged(red_state(0.45, request)));
    assert_eq!(
        moved.outcome,
        ActionOutcome::Applied,
        "те же буквы в другом месте — изменение"
    );
    let closed = screen.step(ScreenAction::ScopeLabelsChanged(ScopeLabelsState::Withdrawn));
    assert_eq!(closed.outcome, ActionOutcome::Applied);
}
