//! Тёплые камеры: когда открываются, кого греют, когда закрываются, что стоит в их запросе.

use camera_contract::zoom::{StandbyDemand, ZoomGesture};

use super::standby_support::*;
use super::*;
use crate::request_controls::{ExposurePlan, FocusPlan};
use crate::standby_status::StandbySlotState;

const OPEN_WARM_CALLS: [&str; 3] = [
    "open_standby_device",
    "configure_standby_session",
    "set_standby_repeating_request",
];

#[test]
fn wheel_demand_warms_the_catalog_neighbour_and_the_primary_is_untouched() {
    let mut rig = StandbyRig::streaming_on_wide(StandbyRigSetup::default());
    let results_before = rig.lens.rig.engine.status().stream.results_completed;

    rig.wheel_at(1.0);

    assert_eq!(
        rig.calls(),
        OPEN_WARM_CALLS,
        "ни close_device, ни повторного open основной"
    );
    assert_eq!(rig.lens.rig.fake.standby_camera(slot(0)), Some(ultra_wide_id()));
    assert_eq!(rig.slot_state(0), StandbySlotState::Warming);
    // Основная стримит как стримила: камера, состояние и счётчики темпа те же.
    assert_eq!(rig.lens.rig.fake.open_camera(), Some(wide_id()));
    assert_eq!(rig.lens.rig.state_name(), "streaming");
    assert_eq!(rig.lens.rig.engine.status().stream.results_completed, results_before);

    rig.make_standby_ready(0);
    assert_eq!(rig.slot_state(0), StandbySlotState::Ready);
    assert_eq!(rig.standby().counters.opened, 1);
    assert_eq!(
        rig.lens.rig.engine.status().stream.results_completed,
        results_before,
        "готовность тёплой — не кадр основной"
    );
}

#[test]
fn standby_generations_come_from_the_shared_counters_and_are_unique() {
    let mut rig = StandbyRig::streaming_on_wide(StandbyRigSetup::default());
    let counters_before = rig.lens.rig.engine.status().counters;
    rig.wheel_at(1.0);
    let counters = rig.lens.rig.engine.status().counters;
    assert_eq!(counters.devices_opened, counters_before.devices_opened + 1);
    assert_eq!(counters.sessions_configured, counters_before.sessions_configured + 1);
    let standby_device = rig
        .lens
        .rig
        .fake
        .standby_device_generation(slot(0))
        .expect("устройство тёплой");
    let standby_session = rig
        .lens
        .rig
        .fake
        .standby_session_generation(slot(0))
        .expect("сессия тёплой");
    assert_ne!(Some(standby_device), rig.lens.rig.fake.current_device_generation());
    assert_ne!(Some(standby_session), rig.lens.rig.fake.current_session_generation());
}

#[test]
fn a_discrete_tap_without_demand_opens_no_standby_and_switches_by_reopening() {
    let mut rig = StandbyRig::streaming_on_wide(StandbyRigSetup::default());
    rig.intent(0.5, ZoomGesture::Discrete, StandbyDemand::NotNeeded);
    let calls = rig.calls();
    assert!(!calls.iter().any(|name| name.contains("standby")), "{calls:?}");
    assert!(
        calls.contains(&"close_device") && calls.contains(&"open_device"),
        "{calls:?}"
    );
    assert_eq!(rig.lens.rig.fake.open_camera(), Some(ultra_wide_id()));
    assert!(rig.standby().slots.is_empty(), "ячейки даже не заводились");
}

#[test]
fn zero_capacity_or_an_unsupporting_backend_means_no_standby_and_a_normal_switch() {
    for (name, config_capacity, backend_capacity) in [
        ("конфиг 0", 0, Some(1)),
        ("бэкенд 0", 1, Some(0)),
        ("бэкенд без тёплых", 1, None),
    ] {
        let mut rig = StandbyRig::streaming_on_wide(StandbyRigSetup {
            with_tele: false,
            max_cameras: config_capacity,
        });
        rig.lens.rig.fake.set_standby_capacity(backend_capacity);
        rig.wheel_at(1.0);
        assert!(rig.calls().is_empty(), "{name}: тёплая не открывалась");
        rig.tap_while_warm(0.5);
        let calls = rig.calls();
        assert!(calls.contains(&"open_device"), "{name}: обычная смена, {calls:?}");
        assert_eq!(rig.lens.rig.fake.open_camera(), Some(ultra_wide_id()), "{name}");
    }
}

#[test]
fn capacity_one_warms_the_closest_neighbour_and_capacity_two_warms_both() {
    let mut rig = StandbyRig::streaming_on_wide(StandbyRigSetup {
        with_tele: true,
        max_cameras: 1,
    });
    rig.wheel_at(1.8);
    assert_eq!(
        rig.lens.rig.fake.standby_camera(slot(0)),
        Some(tele_id()),
        "у 1,8× ближе телевик"
    );
    assert_eq!(rig.standby().slots.len(), 1);

    let mut rig = StandbyRig::streaming_on_wide(StandbyRigSetup {
        with_tele: true,
        max_cameras: 2,
    });
    // Рабочая ёмкость — пересечение конфига (2) и бэкенда: бэкенд на одну ячейку даёт одну.
    rig.wheel_at(1.8);
    assert_eq!(rig.standby().slots.len(), 1, "бэкенд держит одну ячейку");
    let mut rig = StandbyRig::streaming_on_wide(StandbyRigSetup {
        with_tele: true,
        max_cameras: 2,
    });
    rig.lens.rig.fake.set_standby_capacity(Some(2));
    rig.wheel_at(1.8);
    assert_eq!(rig.lens.rig.fake.standby_camera(slot(0)), Some(tele_id()));
    assert_eq!(rig.lens.rig.fake.standby_camera(slot(1)), Some(ultra_wide_id()));
    assert_eq!(
        rig.calls()
            .iter()
            .filter(|name| **name == "open_standby_device")
            .count(),
        2
    );
}

#[test]
fn standby_closes_only_after_the_release_delay_and_a_new_demand_cancels_it() {
    let mut rig = StandbyRig::streaming_on_wide(StandbyRigSetup::default());
    rig.wheel_at(1.0);
    rig.make_standby_ready(0);
    rig.calls();
    let delay = Duration::from_millis(CameraEngineConfig::default().standby.release_delay_ms);

    rig.wheel_closed_at(1.0);
    assert_eq!(rig.slot_state(0), StandbySlotState::Ready, "в паузе камера ещё тёплая");
    assert_eq!(rig.lens.rig.engine.next_deadline(), Some(rig.lens.rig.now + delay));
    rig.lens.rig.advance(delay - Duration::from_millis(1));
    assert!(rig.calls().is_empty());

    // Новый спрос в паузе отменяет закрытие.
    rig.wheel_at(1.0);
    assert_eq!(rig.lens.rig.engine.next_deadline(), None);
    rig.lens.rig.advance(delay * 2);
    assert!(rig.calls().is_empty());
    assert_eq!(rig.slot_state(0), StandbySlotState::Ready);

    // Спрос снят и пауза прошла: закрыто, основная жива.
    rig.wheel_closed_at(1.0);
    rig.lens.rig.advance(delay);
    assert_eq!(rig.calls(), vec!["close_standby"]);
    assert_eq!(rig.slot_state(0), StandbySlotState::Idle);
    assert_eq!(rig.lens.rig.fake.standby_camera(slot(0)), None);
    assert_eq!(rig.lens.rig.state_name(), "streaming");
}

#[test]
fn background_close_and_primary_failure_close_standby_immediately() {
    for (name, trigger) in [
        ("фон", Trigger::Command(CameraCommand::EnterBackground)),
        ("закрытие", Trigger::Command(CameraCommand::Close)),
        ("отключение основной", Trigger::DisconnectPrimary),
    ] {
        let mut rig = StandbyRig::streaming_on_wide(StandbyRigSetup::default());
        rig.wheel_at(1.0);
        rig.make_standby_ready(0);
        rig.calls();
        match trigger {
            Trigger::Command(command) => {
                rig.lens.rig.send(command);
            }
            Trigger::DisconnectPrimary => {
                rig.lens.rig.fake.disconnect_current_device();
                rig.lens.rig.pump();
            }
        }
        let calls = rig.calls();
        assert!(calls.contains(&"close_standby"), "{name}: {calls:?}");
        assert_eq!(rig.lens.rig.fake.standby_camera(slot(0)), None, "{name}");
        assert_eq!(rig.slot_state(0), StandbySlotState::Idle, "{name}");
    }
}

enum Trigger {
    Command(CameraCommand),
    DisconnectPrimary,
}

#[test]
fn standby_request_carries_the_primary_exposure_with_continuous_focus_and_reissues_on_visible_change_only() {
    let mut rig = StandbyRig::streaming_on_wide(StandbyRigSetup::default());
    rig.lens.rig.send(CameraCommand::ApplyControls {
        controls: fully_manual(100, Duration::from_millis(4)),
    });
    rig.wheel_at(1.0);
    let (_, controls) = rig
        .lens
        .rig
        .fake
        .standby_repeating_request(slot(0))
        .expect("запрос тёплой");
    let ExposurePlan::Sensor { exposure_time, iso } = controls.exposure else {
        unreachable!(
            "основная на ручной экспозиции, тёплая должна получить Sensor: {:?}",
            controls.exposure
        );
    };
    let carried = exposure_time.duration().as_secs_f64() * f64::from(iso.value());
    let shift_ev = (carried / (0.004 * 100.0)).log2();
    assert!(
        (shift_ev - 1.22).abs() < 0.03,
        "ширик темнее основной на 1,22 EV, получено {shift_ev}"
    );
    assert_eq!(controls.focus, FocusPlan::Continuous { region: None });
    rig.calls();

    // ISO 100 -> 101: 0,014 EV, ниже порога перепланирования — запрос тёплой не трогаем.
    rig.lens.rig.send(CameraCommand::ApplyControls {
        controls: fully_manual(101, Duration::from_millis(4)),
    });
    assert!(
        !rig.calls().contains(&"set_standby_repeating_request"),
        "мелкий дрейф не должен дёргать тёплую"
    );
    // ISO 101 -> 200: +1 EV — запрос перепоставлен.
    rig.lens.rig.send(CameraCommand::ApplyControls {
        controls: fully_manual(200, Duration::from_millis(4)),
    });
    assert!(rig.calls().contains(&"set_standby_repeating_request"));
    assert_eq!(rig.standby().counters.requests_issued, 2);
}
