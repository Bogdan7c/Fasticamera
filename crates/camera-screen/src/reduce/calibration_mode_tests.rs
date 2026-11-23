//! Тесты режима CALIBRATE как поведения-данных: реестр, смена режима, видоискатель на весь экран, прибор при
//! повороте и смене размера surface, затвор и клавиша громкости (решения 94, 185, 203).

use look_contract::CleanView;
use scope_contract::{ScopeIntent, ScopeKind, ScopeRequest, ScopeScan, VectorscopeAxes};
use shell_contract::orientation::DeviceOrientation;
use shell_contract::surface::SurfaceSize;
use ui_core::geometry::Point;

use crate::action::{KeyPhase, ScreenAction};
use crate::calibration::CalibrationMenu;
use crate::effect::{Effect, ScreenCameraIntent};
use crate::layout::{FramingIntent, NormalizedScreenRect};
use crate::look_test_support::look_facts;
use crate::mode_behaviour::{LayoutPolicy, ModeBehaviour, ShutterPresence, VolumeKeyRole};
use crate::modes::CaptureMode;
use crate::outcome::{ActionOutcome, NoChangeReason, Rejection};
use crate::settings::{AspectRatio, TimerSetting};
use crate::shutter::ShutterState;
use crate::state::PanelState;
use crate::test_support::{OUR_SURFACE, Screen, at_ms};
use crate::tools::{ToolId, ToolOption};

fn shows(effects: &[Effect]) -> Vec<ScopeRequest> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Scopes(ScopeIntent::Show(request)) => Some(*request),
            _ => None,
        })
        .collect()
}

fn hides(effects: &[Effect]) -> usize {
    effects
        .iter()
        .filter(|effect| matches!(effect, Effect::Scopes(ScopeIntent::Hide)))
        .count()
}

fn viewfinder_intents(effects: &[Effect]) -> Vec<&FramingIntent> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Viewfinder(intent) => Some(intent),
            _ => None,
        })
        .collect()
}

#[test]
fn the_carousel_has_photo_and_calibrate_with_their_behaviour_as_data() {
    let screen = Screen::ready();
    let carousel = screen.state.mode_carousel();

    let described: Vec<(&str, CaptureMode, ModeBehaviour)> = carousel
        .iter()
        .map(|mode| (mode.label, mode.mode, mode.behaviour))
        .collect();
    assert_eq!(
        described,
        [
            ("PHOTO", CaptureMode::Photo, ModeBehaviour::PHOTO),
            ("CALIBRATE", CaptureMode::Calibrate, ModeBehaviour::CALIBRATE),
        ]
    );
    assert_eq!(ModeBehaviour::PHOTO.shutter, ShutterPresence::Present);
    assert_eq!(ModeBehaviour::PHOTO.volume_key, VolumeKeyRole::Shoots);
    assert_eq!(ModeBehaviour::PHOTO.layout, LayoutPolicy::PortraitFixed);
    assert_eq!(ModeBehaviour::CALIBRATE.shutter, ShutterPresence::Absent);
    assert_eq!(ModeBehaviour::CALIBRATE.volume_key, VolumeKeyRole::Ignored);
    assert_eq!(ModeBehaviour::CALIBRATE.layout, LayoutPolicy::FollowsOrientation);
    assert!(carousel[1].tools.menu().is_empty() && carousel[1].tools.strip().is_empty());
}

#[test]
fn the_config_flag_removes_calibrate_from_the_registry() {
    let mut config = crate::config::ScreenConfig::default();
    config.calibration.enabled = false;
    let mut screen = Screen::unprepared(config);

    let labels: Vec<&str> = screen.state.mode_carousel().iter().map(|mode| mode.label).collect();
    let transition = screen.step(ScreenAction::ModeSelected(CaptureMode::Calibrate));

    assert_eq!(labels, ["PHOTO"]);
    assert_eq!(transition.outcome, ActionOutcome::Rejected(Rejection::UnknownMode));
    assert_eq!(screen.state.mode(), CaptureMode::Photo);
}

#[test]
fn entering_calibrate_puts_the_viewfinder_on_the_whole_screen_and_asks_for_the_scope() {
    let mut screen = Screen::with_look(look_facts());

    let transition = screen.step(ScreenAction::ModeSelected(CaptureMode::Calibrate));

    assert_eq!(transition.outcome, ActionOutcome::Applied);
    assert_eq!(screen.state.mode(), CaptureMode::Calibrate);
    let framings = viewfinder_intents(&transition.effects);
    let [framing] = framings.as_slice() else {
        panic!("ожидалось одно кадрирование: {:?}", transition.effects);
    };
    assert_eq!(
        framing.destination,
        NormalizedScreenRect {
            left: 0.0,
            top: 0.0,
            width: 1.0,
            height: 1.0
        },
        "видоискатель на весь экран"
    );
    assert_eq!(
        framing.zoom.aspect,
        camera_contract::viewfinder_crop::CropAspect::LongToShort {
            long: OUR_SURFACE.height_px,
            short: OUR_SURFACE.width_px
        },
        "соотношение — экрана, режим cover"
    );
    // Прибор просят размером ровно с его зону: 540 × 743 px на поверхности 1080×2412 (расчёт доль конфига).
    assert_eq!(
        shows(&transition.effects),
        [ScopeRequest {
            kind: ScopeKind::Waveform,
            vectorscope_axes: VectorscopeAxes::YCbCrBt709,
            scan: ScopeScan::Upright,
            image_size: scope_contract::ScopeImageSize::try_new(540, 743).expect("размер"),
        }]
    );
    assert_eq!(hides(&transition.effects), 0);
}

#[test]
fn leaving_calibrate_hides_the_scope_and_returns_the_photo_framing() {
    let mut screen = Screen::calibrating(look_facts());

    let transition = screen.step(ScreenAction::ModeSelected(CaptureMode::Photo));

    assert_eq!(hides(&transition.effects), 1);
    assert!(shows(&transition.effects).is_empty());
    let framings = viewfinder_intents(&transition.effects);
    let layout = screen.state.layout().expect("раскладка");
    assert_eq!(
        framings.as_slice(),
        [&crate::test_support::plain_framing(layout, AspectRatio::FourByThree)]
    );
    assert!(screen.state.calibration().layout().is_none(), "раскладка раздела снята");
    assert_eq!(screen.state.calibration().menu(), CalibrationMenu::Closed);
}

#[test]
fn the_scope_is_not_asked_twice_and_hide_is_not_repeated() {
    let mut screen = Screen::calibrating(look_facts());

    let tick = screen.step(ScreenAction::Tick(at_ms(100)));
    assert!(
        tick.effects.is_empty(),
        "ничего не изменилось — рендеру ничего не шлём: {:?}",
        tick.effects
    );

    screen.step(ScreenAction::ModeSelected(CaptureMode::Photo));
    let again = screen.step(ScreenAction::Tick(at_ms(200)));
    assert!(again.effects.is_empty(), "Hide уже отправлен: {:?}", again.effects);
}

#[test]
fn rotating_the_phone_in_calibrate_resends_the_scope_with_the_new_size_and_scan() {
    let mut screen = Screen::calibrating(look_facts());
    let portrait = screen
        .state
        .calibration()
        .layout()
        .expect("раскладка")
        .scope_image_size();

    let transition = screen.rotate_to(DeviceOrientation::LandscapeLeft);

    let layout = screen.state.calibration().layout().expect("раскладка");
    let requests = shows(&transition.effects);
    assert_eq!(requests.len(), 1, "{:?}", transition.effects);
    assert_eq!(requests[0].scan, ScopeScan::RotatedClockwise);
    assert_eq!(requests[0].image_size, layout.scope_image_size());
    assert_ne!(requests[0].image_size, portrait, "зона прибора в альбоме другая");
    // Видоискатель от поворота не меняется: он по-прежнему на весь портретный экран.
    assert!(viewfinder_intents(&transition.effects).is_empty());

    let back = screen.rotate_to(DeviceOrientation::Portrait);
    let requests = shows(&back.effects);
    assert_eq!(
        (requests[0].scan, requests[0].image_size),
        (ScopeScan::Upright, portrait)
    );
}

#[test]
fn rotating_the_phone_in_photo_sends_nothing_and_keeps_the_portrait_layout() {
    let mut screen = Screen::with_look(look_facts());

    let transition = screen.rotate_to(DeviceOrientation::LandscapeLeft);

    assert!(transition.effects.is_empty(), "{:?}", transition.effects);
    assert!(screen.state.calibration().layout().is_none());
    assert_eq!(
        screen.state.icon_rotation_clockwise_degrees(),
        90.0,
        "значки по-прежнему крутятся"
    );
}

#[test]
fn a_new_surface_size_in_calibrate_resends_the_scope_and_the_viewfinder() {
    let mut screen = Screen::calibrating(look_facts());

    let transition = screen.step(ScreenAction::SurfaceResized(SurfaceSize {
        width_px: 1080,
        height_px: 2340,
    }));

    let layout = screen.state.calibration().layout().expect("раскладка");
    assert_eq!(
        shows(&transition.effects),
        [ScopeRequest {
            kind: ScopeKind::Waveform,
            vectorscope_axes: VectorscopeAxes::YCbCrBt709,
            scan: ScopeScan::Upright,
            image_size: layout.scope_image_size(),
        }]
    );
    assert_ne!(
        (layout.scope_image_size().width(), layout.scope_image_size().height()),
        (540, 743),
        "высота surface другая — зона прибора другая"
    );
    assert_eq!(viewfinder_intents(&transition.effects).len(), 1);
}

#[test]
fn calibrate_chosen_before_the_surface_exists_asks_for_everything_when_it_arrives() {
    let mut screen = Screen::unprepared(crate::config::ScreenConfig::default());
    let early = screen.step(ScreenAction::ModeSelected(CaptureMode::Calibrate));
    assert!(
        early.effects.is_empty(),
        "размера ещё нет: просить нечего {:?}",
        early.effects
    );

    let arrived = screen.step(ScreenAction::SurfaceResized(OUR_SURFACE));

    assert_eq!(viewfinder_intents(&arrived.effects).len(), 1);
    assert_eq!(shows(&arrived.effects).len(), 1);
}

#[test]
fn a_lost_scope_demand_is_asked_again_only_while_calibrating() {
    let mut calibrating = Screen::calibrating(look_facts());
    let again = calibrating.step(ScreenAction::ScopeDemandLost);
    assert_eq!(shows(&again.effects).len(), 1);

    let mut photo = Screen::with_look(look_facts());
    let nothing = photo.step(ScreenAction::ScopeDemandLost);
    assert!(nothing.effects.is_empty());
    assert_eq!(nothing.outcome, ActionOutcome::NoChange(NoChangeReason::ValueUnchanged));
}

#[test]
fn switching_modes_closes_the_controls_menu_and_the_calibration_menu() {
    let mut screen = Screen::with_look(look_facts());
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    assert_eq!(screen.state.panel(), PanelState::ControlsMenu);

    screen.step(ScreenAction::ModeSelected(CaptureMode::Calibrate));
    assert_eq!(screen.state.panel(), PanelState::Closed);

    screen.step(ScreenAction::ControlsMenuOpenRequested);
    assert_eq!(screen.state.calibration().menu(), CalibrationMenu::Root);
    screen.step(ScreenAction::ModeSelected(CaptureMode::Photo));
    assert_eq!(screen.state.calibration().menu(), CalibrationMenu::Closed);
}

#[test]
fn the_calibration_menu_button_opens_the_calibration_menu_not_the_controls_drawer() {
    let mut screen = Screen::calibrating(look_facts());

    let opened = screen.step(ScreenAction::ControlsMenuOpenRequested);
    assert_eq!(opened.outcome, ActionOutcome::Applied);
    assert_eq!(screen.state.calibration().menu(), CalibrationMenu::Root);
    assert_eq!(screen.state.panel(), PanelState::Closed, "меню Controls не открылось");
    assert!(screen.state.is_any_panel_open());

    screen.step(ScreenAction::CalibrationMenuItemTapped(
        crate::calibration::CalibrationMenuItem::Presets,
    ));
    assert_eq!(screen.state.calibration().menu(), CalibrationMenu::Presets);
    screen.step(ScreenAction::SubmenuBackRequested);
    assert_eq!(screen.state.calibration().menu(), CalibrationMenu::Root);
    screen.step(ScreenAction::CalibrationMenuItemTapped(
        crate::calibration::CalibrationMenuItem::ScopeAxes,
    ));
    assert_eq!(screen.state.calibration().menu(), CalibrationMenu::ScopeAxes);
    screen.step(ScreenAction::ControlsMenuCloseRequested);
    assert_eq!(screen.state.calibration().menu(), CalibrationMenu::Closed);
    let nothing = screen.step(ScreenAction::ControlsMenuCloseRequested);
    assert_eq!(nothing.outcome, ActionOutcome::NoChange(NoChangeReason::NothingToClose));
}

#[test]
fn a_menu_item_needs_the_open_root_and_the_calibration_mode() {
    let mut calibrating = Screen::calibrating(look_facts());
    let closed = calibrating.step(ScreenAction::CalibrationMenuItemTapped(
        crate::calibration::CalibrationMenuItem::Presets,
    ));
    assert_eq!(closed.outcome, ActionOutcome::Rejected(Rejection::PanelBlocksAction));

    let mut photo = Screen::with_look(look_facts());
    let wrong_mode = photo.step(ScreenAction::CalibrationMenuItemTapped(
        crate::calibration::CalibrationMenuItem::Presets,
    ));
    assert_eq!(
        wrong_mode.outcome,
        ActionOutcome::Rejected(Rejection::NotInCalibrationMode)
    );
}

#[test]
fn a_tap_on_the_viewfinder_in_calibrate_is_rejected_and_leaves_the_open_menu_alone() {
    let mut screen = Screen::calibrating(look_facts());
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    let before = screen.state.calibration().clone();

    let tap = screen.step(ScreenAction::ViewfinderTapped {
        point: Point::new(100.0, 100.0),
        at: at_ms(10),
    });

    assert_eq!(tap.outcome, ActionOutcome::Rejected(Rejection::NotAvailableInMode));
    assert!(tap.effects.is_empty(), "фокус не наводился: {:?}", tap.effects);
    assert_eq!(screen.state.calibration(), &before, "меню калибровки не тронуто");
    assert_eq!(screen.state.rejections().get(Rejection::NotAvailableInMode), 1);
}

#[test]
fn the_calibration_menu_is_closed_by_the_scrim_request_not_by_a_viewfinder_tap() {
    let mut screen = Screen::calibrating(look_facts());
    screen.step(ScreenAction::ControlsMenuOpenRequested);

    let close = screen.step(ScreenAction::ControlsMenuCloseRequested);

    assert_eq!(close.outcome, ActionOutcome::Applied);
    assert_eq!(screen.state.calibration().menu(), CalibrationMenu::Closed);
}

#[test]
fn the_full_screen_viewfinder_maps_the_whole_surface_to_the_sensor_but_taps_do_not_focus() {
    let mut screen = Screen::calibrating(look_facts());
    let mapping = screen
        .state
        .viewfinder_mapping()
        .expect("перевод координат в режиме cover");
    let size = screen.state.layout().expect("раскладка").screen_size();

    let center = mapping
        .screen_to_sensor(Point::new(size.width / 2.0, size.height / 2.0))
        .expect("центр экрана в кадре");
    assert!(
        (center.x() - 0.5).abs() < 1e-3 && (center.y() - 0.5).abs() < 1e-3,
        "{center:?}"
    );

    // Тап у самого верха экрана: в PHOTO это чёрная полоса над кадром; в CALIBRATE фокуса нет вовсе.
    let corner = Point::new(5.0, 5.0);
    let tap = screen.step(ScreenAction::ViewfinderTapped {
        point: corner,
        at: at_ms(10),
    });
    assert_eq!(tap.outcome, ActionOutcome::Rejected(Rejection::NotAvailableInMode));
    let mut photo = Screen::with_look(look_facts());
    let outside = photo.step(ScreenAction::ViewfinderTapped {
        point: corner,
        at: at_ms(10),
    });
    assert_eq!(
        outside.outcome,
        ActionOutcome::NoChange(NoChangeReason::TapOutsideFrame)
    );
}

#[test]
fn the_shutter_is_absent_in_calibrate_and_returns_in_photo() {
    let mut screen = Screen::calibrating(look_facts());
    assert_eq!(screen.state.shutter_presence(), ShutterPresence::Absent);

    let tap = screen.step(ScreenAction::ShutterTapped { at: at_ms(10) });

    assert_eq!(tap.outcome, ActionOutcome::Rejected(Rejection::ShutterAbsentInMode));
    assert!(tap.effects.is_empty());
    assert_eq!(screen.state.shutter(), ShutterState::Idle);
    assert_eq!(screen.state.rejections().get(Rejection::ShutterAbsentInMode), 1);

    screen.step(ScreenAction::ModeSelected(CaptureMode::Photo));
    let photo = screen.step(ScreenAction::ShutterTapped { at: at_ms(20) });
    assert!(
        photo.effects.iter().any(|effect| matches!(effect, Effect::Capture(_))),
        "{:?}",
        photo.effects
    );
}

#[test]
fn the_volume_key_does_not_shoot_in_calibrate_but_still_shoots_in_photo_even_with_the_menu_open() {
    let mut screen = Screen::calibrating(look_facts());

    let ignored = screen.step(ScreenAction::VolumeKey {
        phase: KeyPhase::Pressed,
        at: at_ms(10),
    });
    assert_eq!(
        ignored.outcome,
        ActionOutcome::NoChange(NoChangeReason::VolumeKeyIgnoredInMode)
    );
    assert!(ignored.effects.is_empty());
    assert_eq!(screen.state.shutter(), ShutterState::Idle);
    assert_eq!(screen.state.rejections().total(), 0, "игнор — не отказ");

    // Решение 94 сохранено: в PHOTO громкость снимает и при открытом меню Controls.
    screen.step(ScreenAction::ModeSelected(CaptureMode::Photo));
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    let shot = screen.step(ScreenAction::VolumeKey {
        phase: KeyPhase::Pressed,
        at: at_ms(20),
    });
    assert!(
        shot.effects.iter().any(|effect| matches!(effect, Effect::Capture(_))),
        "{:?}",
        shot.effects
    );
}

#[test]
fn leaving_photo_cancels_a_running_countdown() {
    let mut screen = Screen::with_look(look_facts());
    screen.step(ScreenAction::ControlsMenuOpenRequested);
    screen.step(ScreenAction::ControlsItemSelected(ToolId::Timer));
    screen.step(ScreenAction::SubmenuOptionSelected(ToolOption::Timer(
        TimerSetting::Seconds3,
    )));
    screen.step(ScreenAction::ControlsMenuCloseRequested);
    screen.step(ScreenAction::ShutterTapped { at: at_ms(10) });
    assert!(matches!(screen.state.shutter(), ShutterState::Countdown { .. }));

    screen.step(ScreenAction::ModeSelected(CaptureMode::Calibrate));
    let late = screen.step(ScreenAction::Tick(at_ms(10_000)));

    assert_eq!(screen.state.shutter(), ShutterState::Idle);
    assert!(
        !late.effects.iter().any(|effect| matches!(effect, Effect::Capture(_))),
        "снимок после выхода из режима стал бы сюрпризом: {:?}",
        late.effects
    );
}

#[test]
fn switching_modes_drops_the_focus_square_and_releases_the_camera_point() {
    let mut screen = Screen::with_look(look_facts());
    screen.tap_center(at_ms(10));

    let switched = screen.step(ScreenAction::ModeSelected(CaptureMode::Calibrate));

    assert!(
        switched
            .effects
            .contains(&Effect::Camera(ScreenCameraIntent::ResetPointOfInterest)),
        "{:?}",
        switched.effects
    );
}

#[test]
fn leaving_calibrate_while_holding_the_clean_view_gives_the_edits_back() {
    let mut screen = Screen::calibrating(look_facts());
    screen.step(ScreenAction::CleanViewPressed);

    let left = screen.step(ScreenAction::ModeSelected(CaptureMode::Photo));

    assert!(
        left.effects.contains(&Effect::CleanView(CleanView::ShowEdited)),
        "{:?}",
        left.effects
    );
    assert_eq!(
        screen.state.calibration().clean_view(),
        crate::calibration::CleanViewState::ShowingEdited
    );
}

#[test]
fn an_invalid_surface_in_calibrate_drops_the_calibration_layout_and_counts_the_rejection() {
    let mut screen = Screen::calibrating(look_facts());
    assert!(screen.state.calibration().layout().is_some());

    let transition = screen.step(ScreenAction::SurfaceResized(SurfaceSize {
        width_px: 1080,
        height_px: 0,
    }));

    assert_eq!(transition.outcome, ActionOutcome::Rejected(Rejection::InvalidSurface));
    assert!(screen.state.layout().is_none());
    assert!(
        screen.state.calibration().layout().is_none(),
        "старая раскладка описывает несуществующую поверхность"
    );
    assert_eq!(screen.state.rejections().get(Rejection::InvalidSurface), 1);

    // Вернулся прежний размер: рендер всё это время держит тот же запрос, повторять его незачем, а видоискатель
    // получает место заново.
    let same_again = screen.step(ScreenAction::SurfaceResized(OUR_SURFACE));
    assert!(shows(&same_again.effects).is_empty(), "{:?}", same_again.effects);
    assert_eq!(viewfinder_intents(&same_again.effects).len(), 1);
    assert!(screen.state.calibration().layout().is_some());
}

#[test]
fn a_persistent_calibration_layout_failure_is_counted_once_and_ticks_stay_no_change() {
    let mut screen = Screen::calibrating(look_facts());
    // Конфиг портится уже после создания состояния: раскладка раздела перестаёт собираться при любой сверке.
    screen.config.calibration.max_scope_image_side_px = 0;

    let first = screen.step(ScreenAction::ScopeKindSelected(ScopeKind::RgbParade));
    assert!(screen.state.calibration().layout().is_none(), "раскладка не собралась");
    assert_eq!(screen.state.rejections().get(Rejection::InvalidSurface), 1);
    assert_ne!(first.outcome, ActionOutcome::Rejected(Rejection::InvalidSurface));

    for tick in 1..=3 {
        let transition = screen.step(ScreenAction::Tick(crate::test_support::at_ms(tick * 100)));
        assert_eq!(
            transition.outcome,
            ActionOutcome::NoChange(NoChangeReason::ValueUnchanged),
            "тик {tick}"
        );
    }
    screen.step(ScreenAction::ScopeKindSelected(ScopeKind::Waveform));
    assert_eq!(
        screen.state.rejections().get(Rejection::InvalidSurface),
        1,
        "та же ошибка не считается заново"
    );
}
