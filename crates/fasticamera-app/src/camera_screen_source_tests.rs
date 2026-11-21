//! Снимок наблюдаемого состояния экрана ([`ScreenStateSnapshot`]): его значения должны ПОВТОРЯТЬ настоящее
//! состояние `ScreenState`, которое меняют настоящие действия редьюсера (а не подставленные руками поля).

use camera_contract::control::ExposureBias;
use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_screen::tools::ToolId;
use camera_screen::viewfinder_map::{MirrorMode, StreamGeometry};
use camera_screen::{CameraFacts, CameraPresence, ScreenAction, ScreenConfig, ScreenState, reduce};
use shell_contract::clock::MonotonicNanos;
use shell_contract::surface::SurfaceSize;

use super::*;
use crate::camera_screen_capture::{CaptureService, SubmitOutcome};
use crate::camera_screen_effects::{CameraIntentOutcome, CameraIntentPort, EffectTargets};
use crate::camera_screen_facts::FactsDiagnostics;
use crate::camera_screen_input::screen_input_channel;
use crate::gpu_test_support::RecordingRenderControl;
use camera_core::CommandId;
use camera_screen::ScreenCameraIntent;
use camera_screen::shutter::ShotRequest;
use shell_contract::commands::{CommandOutcome, FrameRate, FrameRateOutcome, ScreenWake, ShellCommands};
use shell_contract::haptics::{HapticOutcome, HapticPattern, HapticsService};
use std::sync::Mutex;
use ui_draw::geometry::PxSize;

/// Применяет действие редьюсером к состоянию и возвращает новое состояние.
fn apply(state: &ScreenState, action: ScreenAction) -> ScreenState {
    reduce(state, action, &ScreenConfig::default()).state
}

/// Экран с живой камерой (без неё EV-перетаскивание отвергается).
fn live_screen() -> ScreenState {
    let state = ScreenState::new(&ScreenConfig::default()).expect("конфиг по умолчанию верен");
    let facts = CameraFacts {
        presence: CameraPresence::Live,
        // Основная камера OnePlus 11: без геометрии потока тап не переводится в точку сенсора.
        stream: Some(StreamGeometry {
            valid_area: PixelRect {
                left: 0,
                top: 0,
                size: PixelSize::new(4096, 3072),
            },
            active_array_size: PixelSize::new(4096, 3072),
            sensor_orientation: SensorOrientation::Degrees90,
            mirror: MirrorMode::None,
        }),
        ..CameraFacts::STARTING
    };
    let state = apply(&state, ScreenAction::CameraFactsChanged(facts));
    // Без размера surface нет раскладки, и тап по видоискателю некуда привязать.
    apply(
        &state,
        ScreenAction::SurfaceResized(SurfaceSize {
            width_px: 1080,
            height_px: 2412,
        }),
    )
}

#[test]
fn a_fresh_screen_is_closed_neutral_and_in_photo_mode() {
    let snapshot = ScreenStateSnapshot::observe(&live_screen());
    assert_eq!(
        snapshot,
        ScreenStateSnapshot {
            panel: PanelKind::Closed,
            lock_plaque: Presence::Hidden,
            focus_indicator: Presence::Hidden,
            exposure_steps: 0,
            mode: ModeKind::Photo,
            shutter_placement: ShutterPlacement::Visible,
            shutter_activity: ShutterActivity::Idle,
            zoom: ZoomView {
                ratio_milli: 1_000,
                facing: camera_contract::ids::LensFacing::Back,
                wheel_open: false,
                gliding: false,
            },
        }
    );
}

#[test]
fn the_open_controls_menu_hides_the_shutter_and_closing_brings_it_back() {
    let opened = apply(&live_screen(), ScreenAction::ControlsMenuOpenRequested);
    let snapshot = ScreenStateSnapshot::observe(&opened);
    assert_eq!(snapshot.panel, PanelKind::ControlsMenu);
    assert_eq!(snapshot.shutter_placement, ShutterPlacement::HiddenUnderDrawer);

    let closed = apply(&opened, ScreenAction::ControlsMenuCloseRequested);
    let snapshot = ScreenStateSnapshot::observe(&closed);
    assert_eq!(snapshot.panel, PanelKind::Closed);
    assert_eq!(snapshot.shutter_placement, ShutterPlacement::Visible);
}

#[test]
fn an_open_tool_card_raises_the_shutter_whichever_way_it_was_opened_and_closing_it_returns_the_shutter() {
    // Карточка Aspect, открытая из меню: затвор поднят над карточкой (виден и снимает), а не спрятан под шторку.
    let menu = apply(&live_screen(), ScreenAction::ControlsMenuOpenRequested);
    let card_from_menu = apply(&menu, ScreenAction::ControlsItemSelected(ToolId::Aspect));
    let snapshot = ScreenStateSnapshot::observe(&card_from_menu);
    assert_eq!(snapshot.panel, PanelKind::Submenu);
    assert_eq!(snapshot.shutter_placement, ShutterPlacement::RaisedAboveCard);

    // Карточка Exposure со значка над кадром: тот же поднятый затвор.
    let card_from_icon = apply(&live_screen(), ScreenAction::StripToolTapped(ToolId::Exposure));
    let snapshot = ScreenStateSnapshot::observe(&card_from_icon);
    assert_eq!(snapshot.panel, PanelKind::ExposurePanel);
    assert_eq!(snapshot.shutter_placement, ShutterPlacement::RaisedAboveCard);

    // «×» закрывает карточку: затвор возвращается на место.
    let closed = apply(&card_from_icon, ScreenAction::SubmenuBackRequested);
    let snapshot = ScreenStateSnapshot::observe(&closed);
    assert_eq!(snapshot.panel, PanelKind::Closed);
    assert_eq!(snapshot.shutter_placement, ShutterPlacement::Visible);
}

#[test]
fn the_exposure_panel_and_the_ev_steps_are_visible_in_the_snapshot_and_reset_returns_zero() {
    let panel = apply(&live_screen(), ScreenAction::StripToolTapped(ToolId::Exposure));
    assert_eq!(ScreenStateSnapshot::observe(&panel).panel, PanelKind::ExposurePanel);

    let dragged = apply(
        &panel,
        ScreenAction::ExposureDragged {
            requested: ExposureBias::from_ev(1.0).expect("конечное значение"),
            at: MonotonicNanos::from_nanos(0),
        },
    );
    assert_eq!(ScreenStateSnapshot::observe(&dragged).exposure_steps, 3);

    let reset = apply(&dragged, ScreenAction::ExposureResetRequested);
    assert_eq!(ScreenStateSnapshot::observe(&reset).exposure_steps, 0);
}

#[test]
fn a_viewfinder_tap_shows_the_focus_indicator_without_the_lock_plaque() {
    let tapped = apply(
        &live_screen(),
        ScreenAction::ViewfinderTapped {
            point: ui_core::geometry::Point::new(200.0, 400.0),
            at: MonotonicNanos::from_nanos(1_000_000),
        },
    );
    let snapshot = ScreenStateSnapshot::observe(&tapped);
    assert_eq!(snapshot.focus_indicator, Presence::Shown);
    assert_eq!(snapshot.lock_plaque, Presence::Hidden);
}

#[test]
fn the_snapshot_serializes_for_the_run_report_with_readable_names() {
    let json = serde_json::to_value(ScreenStateSnapshot::observe(&live_screen())).expect("снимок сериализуется");
    assert_eq!(json["panel"], "Closed");
    assert_eq!(json["shutter_placement"], "Visible");
    assert_eq!(json["exposure_steps"], 0);
}

// ---- Проводка источника: досылка EV по кадрам (находка 4) ---------------------------------------------------------

struct LiveFacts;

impl CameraFactsSource for LiveFacts {
    fn current_facts(&mut self) -> CameraFacts {
        CameraFacts {
            presence: CameraPresence::Live,
            stream: Some(StreamGeometry {
                valid_area: PixelRect {
                    left: 0,
                    top: 0,
                    size: PixelSize::new(4096, 3072),
                },
                active_array_size: PixelSize::new(4096, 3072),
                sensor_orientation: SensorOrientation::Degrees90,
                mirror: MirrorMode::None,
            }),
            ..CameraFacts::STARTING
        }
    }

    fn diagnostics(&self) -> FactsDiagnostics {
        FactsDiagnostics::default()
    }
}

/// Порт камеры, ответ которого тест меняет по ходу кадров.
struct SwitchablePort {
    outcome: Arc<Mutex<CameraIntentOutcome>>,
    received: Arc<Mutex<Vec<ScreenCameraIntent>>>,
}

impl CameraIntentPort for SwitchablePort {
    fn deliver(&self, intent: ScreenCameraIntent) -> CameraIntentOutcome {
        self.received.lock().expect("замок").push(intent);
        *self.outcome.lock().expect("замок")
    }
}

struct QuietHaptics;

impl HapticsService for QuietHaptics {
    fn play(&self, _pattern: HapticPattern) -> HapticOutcome {
        HapticOutcome::Unsupported
    }
}

struct QuietCommands;

impl ShellCommands for QuietCommands {
    fn request_camera_permission(&self) -> CommandOutcome {
        CommandOutcome::Queued
    }

    fn set_preferred_frame_rate(&self, _rate: FrameRate) -> FrameRateOutcome {
        FrameRateOutcome::Applied
    }

    fn open_system_gallery(&self) -> CommandOutcome {
        CommandOutcome::Queued
    }

    fn keep_screen_on(&self, _wake: ScreenWake) -> CommandOutcome {
        CommandOutcome::Queued
    }
}

struct NoCapture;

impl CaptureService for NoCapture {
    fn submit(&self, _request: ShotRequest, _now: MonotonicNanos) -> SubmitOutcome {
        SubmitOutcome::Accepted
    }
}

fn frame_request(now_ms: i64) -> UiFrameRequest {
    UiFrameRequest {
        present_time: MonotonicNanos::from_nanos(now_ms * 1_000_000),
        logical_size: PxSize::new(1080.0, 2412.0),
        px_per_pt: 1080.0 / 440.0,
        backdrop: ui_draw::frame_source::BackdropContinuity::Live,
    }
}

#[test]
fn an_exposure_bias_refused_by_a_full_camera_queue_is_resent_by_the_source_on_later_frames() {
    let outcome = Arc::new(Mutex::new(CameraIntentOutcome::QueueFull));
    let received = Arc::new(Mutex::new(Vec::new()));
    let (sender, receiver) = screen_input_channel(16);
    let board = Arc::new(ScreenBoard::default());
    let mut source = CameraScreenSource::new(ScreenSourceParts {
        screen: CameraScreen::new(
            ScreenConfig::default(),
            ui_kit::tokens::UiKitTokens::default(),
            ui_core::time::MotionPreference::Full,
        )
        .expect("экран"),
        input: receiver,
        facts: Box::new(LiveFacts),
        gravity: GravityLink::Unavailable {
            reason: "тест".to_owned(),
        },
        executor: EffectExecutor::new(EffectTargets {
            camera: Box::new(SwitchablePort {
                outcome: Arc::clone(&outcome),
                received: Arc::clone(&received),
            }),
            haptics: Arc::new(QuietHaptics),
            commands: Arc::new(QuietCommands),
            render: RecordingRenderControl::new(),
            capture: Arc::new(NoCapture),
            lens: camera_contract::zoom::LensIntentChannel::new(),
        }),
        board: Arc::clone(&board),
        report_min_interval: Duration::ZERO,
    });

    // Кадр 1: сцена построена, факты Live доставлены. Кадр 2: панель Exposure и перетаскивание шкалы.
    source.produce_frame(&frame_request(16)).expect("кадр");
    let bias = ExposureBias::from_ev(1.0).expect("EV");
    sender.send_action(ScreenAction::StripToolTapped(ToolId::Exposure));
    sender.send_action(ScreenAction::ExposureDragged {
        requested: bias,
        at: MonotonicNanos::from_nanos(20_000_000),
    });
    source.produce_frame(&frame_request(32)).expect("кадр");
    let intents = |received: &Arc<Mutex<Vec<ScreenCameraIntent>>>| received.lock().expect("замок").clone();
    assert_eq!(
        intents(&received),
        vec![ScreenCameraIntent::SetExposureBias(bias)],
        "первая отправка упёрлась в очередь"
    );
    let report = board.latest().expect("отчёт");
    assert_eq!((report.effects.camera.queue_full, report.effects.camera.sent), (1, 0));

    // Очередь движка освободилась: следующий кадр досылает то же значение сам, без нового жеста пользователя.
    *outcome.lock().expect("замок") = CameraIntentOutcome::Sent(CommandId(5));
    source.produce_frame(&frame_request(48)).expect("кадр");
    assert_eq!(
        intents(&received),
        vec![
            ScreenCameraIntent::SetExposureBias(bias),
            ScreenCameraIntent::SetExposureBias(bias)
        ],
        "значение досылается"
    );
    assert_eq!(
        board.latest().expect("отчёт").effects.camera.exposure_resends_accepted,
        1
    );
    // Принято — больше не досылается.
    source.produce_frame(&frame_request(64)).expect("кадр");
    assert_eq!(intents(&received).len(), 2);
}

/// Экран с каталогом зума OnePlus 11 и основной камерой: зум-действия принимаются.
fn zoom_screen() -> ScreenState {
    use camera_contract::fixtures::camera_id;
    use camera_contract::ids::{LensFacing, LensRole};
    let state = live_screen();
    let facts = CameraFacts {
        lens: camera_screen::LensFacts {
            catalog: Some(crate::camera_test_support::oneplus_zoom_catalog()),
            active: Some(camera_screen::ActiveLens {
                camera: camera_id("2"),
                role: LensRole::Wide,
                facing: LensFacing::Back,
            }),
            ..camera_screen::LensFacts::NONE
        },
        ..CameraFacts::STARTING
    };
    apply(
        &state,
        ScreenAction::CameraFactsChanged(CameraFacts {
            presence: CameraPresence::Live,
            stream: Some(StreamGeometry {
                valid_area: PixelRect {
                    left: 0,
                    top: 0,
                    size: PixelSize::new(4096, 3072),
                },
                active_array_size: PixelSize::new(4096, 3072),
                sensor_orientation: SensorOrientation::Degrees90,
                mirror: MirrorMode::None,
            }),
            ..facts
        }),
    )
}

#[test]
fn zoom_view_follows_the_real_zoom_state_ratio_facing_and_wheel() {
    use camera_contract::ids::LensFacing;
    use camera_contract::zoom::ZoomRatio;
    let at = MonotonicNanos::from_nanos(1_000_000);
    let state = zoom_screen();
    let initial = ScreenStateSnapshot::observe(&state).zoom;
    assert_eq!(initial.ratio_milli, 1_000, "до выбора пользователя экран показывает 1×");
    assert_eq!((initial.facing, initial.wheel_open), (LensFacing::Back, false));

    let touched = apply(&state, ScreenAction::ZoomWheelTouched { at });
    assert!(
        ScreenStateSnapshot::observe(&touched).zoom.wheel_open,
        "палец на колесе — колесо открыто"
    );

    let dragged = apply(
        &touched,
        ScreenAction::ZoomWheelDragged {
            requested: ZoomRatio::new(1.7).expect("коэффициент"),
            at,
        },
    );
    assert_eq!(ScreenStateSnapshot::observe(&dragged).zoom.ratio_milli, 1_700);

    let flipped = apply(&dragged, ScreenAction::FlipCameraRequested { at });
    let view = ScreenStateSnapshot::observe(&flipped).zoom;
    assert_eq!(view.facing, LensFacing::Front, "переворот меняет направление в снимке");
    assert!(!view.wheel_open, "переворот закрывает колесо");
}

#[test]
fn dropping_the_source_releases_the_warm_demand_the_screen_left_in_the_lens_slot() {
    use camera_contract::ids::LensFacing;
    use camera_contract::zoom::{LensIntent, LensIntentChannel, StandbyDemand, ZoomGesture, ZoomRatio};

    let lens = LensIntentChannel::new();
    let (_sender, receiver) = screen_input_channel(16);
    let source = CameraScreenSource::new(ScreenSourceParts {
        screen: CameraScreen::new(
            ScreenConfig::default(),
            ui_kit::tokens::UiKitTokens::default(),
            ui_core::time::MotionPreference::Full,
        )
        .expect("экран"),
        input: receiver,
        facts: Box::new(LiveFacts),
        gravity: GravityLink::Unavailable {
            reason: "тест".to_owned(),
        },
        executor: EffectExecutor::new(EffectTargets {
            camera: Box::new(crate::camera_screen_effects::AbsentCameraPort),
            haptics: Arc::new(QuietHaptics),
            commands: Arc::new(QuietCommands),
            render: RecordingRenderControl::new(),
            capture: Arc::new(NoCapture),
            lens: lens.clone(),
        }),
        board: Arc::new(ScreenBoard::default()),
        report_min_interval: Duration::ZERO,
    });
    let warm = LensIntent::without_standby(
        LensFacing::Back,
        ZoomRatio::new(1.4).expect("коэффициент"),
        ZoomGesture::Continuous,
    )
    .with_standby(StandbyDemand::KeepNeighbourWarm);
    lens.publish(warm);

    // Экран снят (сценарий, рендер, переустановка): источник уничтожен вместе с исполнителем.
    drop(source);

    let (_, latest) = lens.latest().expect("намерение");
    assert_eq!(latest, warm.with_standby(StandbyDemand::NotNeeded));
}
