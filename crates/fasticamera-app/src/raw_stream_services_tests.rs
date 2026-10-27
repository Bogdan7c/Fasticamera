use std::sync::Mutex;

use super::*;

/// Счётчики, которые тест выставляет вручную.
#[derive(Default)]
struct ScriptedCounters(Mutex<RawStreamCounters>);

impl RawStreamCounterSource for ScriptedCounters {
    fn counters(&self) -> RawStreamCounters {
        *self.0.lock().expect("замок")
    }
}

#[test]
fn detached_services_have_zero_counters_and_a_disarmed_probe() {
    let services = RawStreamServices::detached();
    assert_eq!(services.counters(), RawStreamCounters::default());
    assert_eq!(services.cpu_probe_mode(), CpuProbeMode::Disarmed);
}

#[test]
fn counters_follow_the_underlying_source_live() {
    let source = Arc::new(ScriptedCounters::default());
    let services = RawStreamServices::new(
        source.clone(),
        CpuAccessProbe::disarmed(),
        Arc::new(SystemClocks),
        RawFrameStreamConfig::default(),
    );
    assert_eq!(services.counters().frames_offered, 0);
    source.0.lock().expect("замок").frames_offered = 17;
    assert_eq!(services.counters().frames_offered, 17);
}

#[test]
fn probe_switch_is_shared_with_the_backend_handle_and_can_be_disarmed_twice() {
    // Бэкенд держит клон той же ручки: переключение сервиса должно быть ему видно.
    let backend_handle = CpuAccessProbe::disarmed();
    let services = RawStreamServices::new(
        Arc::new(NoStreamCounters),
        backend_handle.clone(),
        Arc::new(SystemClocks),
        RawFrameStreamConfig::default(),
    );
    services.arm_cpu_probe();
    assert_eq!(backend_handle.mode(), CpuProbeMode::CopyFirstRow);
    services.disarm_cpu_probe();
    services.disarm_cpu_probe();
    assert_eq!(backend_handle.mode(), CpuProbeMode::Disarmed);
}

#[test]
fn pool_plan_upper_bound_follows_the_stream_config() {
    let config = RawFrameStreamConfig {
        viewfinder_render_holds: 2,
        max_pending_frames: 3,
        pool_slack: 1,
        snapshot_ring_budget_bytes: 0,
        snapshot_ring_max_frames: 30,
        ..RawFrameStreamConfig::default()
    };
    let services = RawStreamServices::new(
        Arc::new(NoStreamCounters),
        CpuAccessProbe::disarmed(),
        Arc::new(SystemClocks),
        config.clone(),
    );
    let plan = services.pool_plan_upper_bound();
    assert_eq!(plan.viewfinder, 3, "ячейка + два кадра у рендера");
    assert_eq!(plan.snapshot_ring, 0, "нулевой бюджет — кольца нет");
    assert_eq!(plan.reader_max_images(), 3 + 3 + 1);

    let with_ring = RawStreamServices::new(
        Arc::new(NoStreamCounters),
        CpuAccessProbe::disarmed(),
        Arc::new(SystemClocks),
        RawFrameStreamConfig {
            snapshot_ring_budget_bytes: 1,
            ..config
        },
    );
    assert_eq!(with_ring.pool_plan_upper_bound().snapshot_ring, 30);
}
