//! Упоры ручек теплоты и оттенка доходят до экрана (решения 288, 293): рендер кладёт значение на доску, источник кадров
//! отправляет его экрану ровно один раз на изменение, экран хранит и отдаёт шкалам.

use look_contract::{
    GlobalKnob, KnobSideStops, KnobStop, KnobTravelStops, LookWhiteStops, LookWhiteStopsBoard, StopFraction,
};

use super::*;
use crate::look_white_stops::LookWhiteStopsWatcher;

fn tint_plus_stop(fraction: f32) -> LookWhiteStops {
    let none = KnobSideStops {
        minus: KnobStop::None,
        plus: KnobStop::None,
    };
    LookWhiteStops::Known(KnobTravelStops {
        warmth: none,
        tint: KnobSideStops {
            plus: KnobStop::At(StopFraction::new(fraction).expect("доля в (0, 1]")),
            ..none
        },
    })
}

fn source_over(board: &LookWhiteStopsBoard) -> (CameraScreenSource, Arc<ScreenBoard>) {
    let (_sender, receiver) = screen_input_channel(16);
    let report = Arc::new(ScreenBoard::default());
    let source = CameraScreenSource::new(ScreenSourceParts {
        look_white_stops: LookWhiteStopsWatcher::new(board.clone()),
        scope_labels: crate::scope_labels::ScopeLabelsWatcher::silent(),
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
fn stops_published_by_the_render_reach_the_screen_once_and_unchanged_stops_are_not_resent() {
    let board = LookWhiteStopsBoard::new();
    let (mut source, report) = source_over(&board);
    source.produce_frame(&frame_request(16)).expect("кадр");
    assert_eq!(
        report.latest().expect("отчёт").look_white_stops_sent,
        0,
        "рендер молчит — экрану нечего слать"
    );
    assert_eq!(
        source.screen_state().look().knob_stops(GlobalKnob::Tint).plus,
        KnobStop::None
    );

    board.publish(tint_plus_stop(0.46));
    source.produce_frame(&frame_request(32)).expect("кадр");
    assert_eq!(report.latest().expect("отчёт").look_white_stops_sent, 1);
    assert_eq!(
        source.screen_state().look().knob_stops(GlobalKnob::Tint).plus,
        KnobStop::At(StopFraction::new(0.46).expect("доля")),
        "экран получил упор оттенка"
    );

    for frame in 3..=40 {
        source.produce_frame(&frame_request(16 * frame)).expect("кадр");
    }
    board.publish(tint_plus_stop(0.46));
    source.produce_frame(&frame_request(700)).expect("кадр");
    assert_eq!(
        report.latest().expect("отчёт").look_white_stops_sent,
        1,
        "тихие кадры и повторная публикация того же значения экрану не отправляются"
    );

    board.publish(LookWhiteStops::NotApplicable);
    source.produce_frame(&frame_request(716)).expect("кадр");
    assert_eq!(report.latest().expect("отчёт").look_white_stops_sent, 2);
    assert_eq!(
        source.screen_state().look().knob_stops(GlobalKnob::Tint).plus,
        KnobStop::None,
        "«не применимо» убрало отметки"
    );
}
