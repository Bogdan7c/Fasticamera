//! Подписи прибора доходят до источника экрана (решения 326, 327): рендер кладёт значение на доску, источник кадров
//! отправляет его экрану ровно один раз на изменение и считает отправки в отчёте.

use scope_contract::{
    LabelPoint, ScopeImageSize, ScopeKind, ScopeLabel, ScopeLabels, ScopeLabelsBoard, ScopeLabelsState, ScopeMark,
    ScopeRequest, ScopeScan, TargetColor, VectorscopeAxes, VectorscopeZoom,
};

use super::*;
use crate::scope_labels::ScopeLabelsWatcher;

fn red_label_state() -> ScopeLabelsState {
    ScopeLabelsState::Published(ScopeLabels {
        request: ScopeRequest {
            kind: ScopeKind::Vectorscope,
            vectorscope_axes: VectorscopeAxes::YCbCrBt709,
            vectorscope_zoom: VectorscopeZoom::Normal,
            cie_diagram: scope_contract::CieDiagram::default(),
            histogram_mode: scope_contract::HistogramMode::default(),
            scan: ScopeScan::Upright,
            image_size: ScopeImageSize::try_new(300, 300).expect("размер в допуске"),
        },
        labels: vec![ScopeLabel {
            mark: ScopeMark::VectorscopeTarget(TargetColor::Red),
            at: LabelPoint::try_new(0.3, 0.2).expect("точка в картинке"),
        }],
    })
}

fn source_over(board: &ScopeLabelsBoard) -> (CameraScreenSource, Arc<ScreenBoard>) {
    let (_sender, receiver) = screen_input_channel(16);
    let report = Arc::new(ScreenBoard::default());
    let source = CameraScreenSource::new(ScreenSourceParts {
        look_white_stops: LookWhiteStopsWatcher::silent(),
        scope_labels: ScopeLabelsWatcher::new(board.clone()),
        screen: CameraScreen::new(
            ScreenConfig::default(),
            ui_kit::tokens::UiKitTokens::default(),
            ui_core::time::MotionPreference::Full,
        )
        .expect("экран"),
        input: receiver,
        facts: Box::new(LiveFacts),
        look_facts: None,
        gravity: GravityLink::Unavailable {
            reason: "тест".to_owned(),
        },
        executor: EffectExecutor::new(EffectTargets {
            camera: Box::new(SwitchablePort {
                outcome: Arc::new(Mutex::new(CameraIntentOutcome::NoEngine)),
                received: Arc::default(),
            }),
            haptics: Arc::new(QuietHaptics),
            commands: Arc::new(QuietCommands),
            render: RecordingRenderControl::new(),
            capture: Arc::new(NoCapture),
            shutter_sound: Arc::new(crate::camera_screen_service::SilentShutterSound::default()),
            lens: camera_contract::zoom::LensIntentChannel::new(),
            look: None,
        }),
        board: Arc::clone(&report),
        report_min_interval: Duration::ZERO,
    });
    (source, report)
}

#[test]
fn labels_published_by_the_render_reach_the_screen_once_and_unchanged_ones_are_not_resent() {
    let board = ScopeLabelsBoard::new();
    let (mut source, report) = source_over(&board);
    source.produce_frame(&frame_request(16)).expect("кадр");
    assert_eq!(
        report.latest().expect("отчёт").scope_labels_sent,
        0,
        "рендер молчит — экрану нечего слать"
    );

    board.publish(red_label_state());
    source.produce_frame(&frame_request(32)).expect("кадр");
    assert_eq!(report.latest().expect("отчёт").scope_labels_sent, 1);

    for frame in 3..=40 {
        source.produce_frame(&frame_request(16 * frame)).expect("кадр");
    }
    board.publish(red_label_state());
    source.produce_frame(&frame_request(700)).expect("кадр");
    assert_eq!(
        report.latest().expect("отчёт").scope_labels_sent,
        1,
        "тихие кадры и повторная публикация того же значения экрану не отправляются"
    );

    board.publish(ScopeLabelsState::Withdrawn);
    source.produce_frame(&frame_request(716)).expect("кадр");
    assert_eq!(
        report.latest().expect("отчёт").scope_labels_sent,
        2,
        "закрытие приборов — тоже изменение"
    );
}
