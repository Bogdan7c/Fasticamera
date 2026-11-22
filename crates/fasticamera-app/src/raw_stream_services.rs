//! Что сценарии знают о потоке RAW-кадров: счётчики, план пула, рубильник пробы CPU, часы.
//!
//! Composition root создаёт ОДИН `RawFrameStream` и раздаёт его концы: `producer()` — камере,
//! `viewfinder()` — рендеру. Сценариям нужен четвёртый, наблюдательный вход — этот сервис. Он даёт
//! намерения («сними счётчики», «взведи пробу»), а не сам поток: сценарии не могут ни выдать
//! кадры, ни вернуть их.
//!
//! Все источники нейтральны к платформе: на телефоне за ними стоят `RawFrameStream`, проба
//! `camera-android` и системные часы, в тестах — fake-объекты.

use std::sync::Arc;

use camera_android::{CpuAccessProbe, CpuProbeMode};
use raw_frame_stream::{
    PoolPlan, RawFrameStream, RawFrameStreamConfig, RawStreamCounters, ViewfinderPacing, ViewfinderPacingSwitch,
};

use crate::clock_offset::{ClockOffset, ClockOffsetSource, ClockReadError, SystemClocks};
use crate::viewfinder_pacing_control::{LocalPacing, PacingClaim, ViewfinderPacingControl};

/// Откуда берутся счётчики потока кадров (на телефоне — `RawFrameStream`, в тестах — fake).
pub trait RawStreamCounterSource: Send + Sync {
    fn counters(&self) -> RawStreamCounters;
}

impl RawStreamCounterSource for RawFrameStream {
    fn counters(&self) -> RawStreamCounters {
        RawFrameStream::counters(self)
    }
}

/// Источник счётчиков, когда потока нет (сборка без камеры): всегда нули.
#[derive(Debug, Clone, Copy, Default)]
struct NoStreamCounters;

impl RawStreamCounterSource for NoStreamCounters {
    fn counters(&self) -> RawStreamCounters {
        RawStreamCounters::default()
    }
}

/// Наблюдательные сервисы потока кадров. Дёшево клонируется.
#[derive(Clone)]
pub struct RawStreamServices {
    counters: Arc<dyn RawStreamCounterSource>,
    cpu_probe: CpuAccessProbe,
    clocks: Arc<dyn ClockOffsetSource>,
    /// Конфиг потока: из него считается потолок плана пула (`pool_plan_upper_bound`).
    stream_config: RawFrameStreamConfig,
    /// Переключатель политики подачи кадров видоискателя (решение 53).
    pacing: Arc<dyn ViewfinderPacingControl>,
    /// Политика, действовавшая до вмешательства сценария (общая на клоны сервиса).
    pacing_claim: Arc<PacingClaim>,
}

impl RawStreamServices {
    pub fn new(
        counters: Arc<dyn RawStreamCounterSource>,
        cpu_probe: CpuAccessProbe,
        clocks: Arc<dyn ClockOffsetSource>,
        stream_config: RawFrameStreamConfig,
    ) -> Self {
        // Пока не подключён живой поток, политика — локальная, стартует со значения из конфига.
        let pacing = Arc::new(LocalPacing::new(stream_config.viewfinder_pacing));
        Self {
            counters,
            cpu_probe,
            clocks,
            stream_config,
            pacing,
            pacing_claim: Arc::default(),
        }
    }

    /// Подключает настоящий переключатель политики (composition root: живой `RawFrameStream`).
    pub fn with_pacing_control(mut self, pacing: Arc<dyn ViewfinderPacingControl>) -> Self {
        self.pacing = pacing;
        self
    }

    /// Действующая политика подачи кадров видоискателя.
    pub fn viewfinder_pacing(&self) -> ViewfinderPacing {
        self.pacing.pacing()
    }

    /// Сценарий: подать кадры видоискателя этой политикой. Исходная политика запоминается один раз
    /// и возвращается [`RawStreamServices::restore_viewfinder_pacing_after_scenario`] при любом конце сценария.
    pub fn set_viewfinder_pacing_for_scenario(&self, pacing: ViewfinderPacing) -> ViewfinderPacingSwitch {
        self.pacing_claim.remember_original(self.pacing.pacing());
        self.pacing.switch_to(pacing)
    }

    /// Конец сценария: вернуть политику, бывшую до него. `None` — сценарий политику не трогал.
    /// Вызывает только `CameraServices::scenario_ended`.
    pub fn restore_viewfinder_pacing_after_scenario(&self) -> Option<ViewfinderPacingSwitch> {
        let original = self.pacing_claim.take_original()?;
        Some(self.pacing.switch_to(original))
    }

    /// Сервис без живого потока: счётчики нулевые, проба выключена и никому не подключена.
    /// Для сборок без камеры и тестов, которым поток не нужен.
    pub fn detached() -> Self {
        Self::detached_with(RawFrameStreamConfig::default())
    }

    /// То же, что [`RawStreamServices::detached`], но с заданным конфигом потока.
    pub fn detached_with(stream_config: RawFrameStreamConfig) -> Self {
        Self::new(
            Arc::new(NoStreamCounters),
            CpuAccessProbe::disarmed(),
            Arc::new(SystemClocks),
            stream_config,
        )
    }

    /// Накопительные счётчики потока кадров.
    pub fn counters(&self) -> RawStreamCounters {
        self.counters.counters()
    }

    /// Потолок плана пула, который бэкенд может выбрать при конфигурации потока.
    ///
    /// Точный план зависит от размера кадра (кольцо снимков = `min(бюджет / размер, потолок)`),
    /// которого сервис не знает, поэтому кольцо берётся по верхней оценке: нулевой бюджет — кольца
    /// нет, иначе потолок числа кадров. Очереди «ни одного молча» сейчас не подключаются (0).
    /// Сверка «кадров на руках не больше плана» с верхней оценкой не даёт ложных провалов и всё
    /// равно ловит утечку: утёкший лизинг растёт без предела.
    pub fn pool_plan_upper_bound(&self) -> PoolPlan {
        let config = &self.stream_config;
        PoolPlan {
            viewfinder: config.viewfinder_pool_cells(),
            pending_pairs: config.max_pending_frames,
            snapshot_ring: if config.snapshot_ring_budget_bytes == 0 {
                0
            } else {
                config.snapshot_ring_max_frames
            },
            // Удержания снимков считаются только при включённом кольце (`reader_max_images` это учитывает).
            snapshot_capture_holds: config.snapshot_capture_holds,
            lossless: 0,
            slack: config.pool_slack,
        }
    }

    /// Взводит отладочную пробу CPU-доступа. Включать ДО конфигурации сессии: usage читателя
    /// выбирается при его создании.
    pub fn arm_cpu_probe(&self) {
        self.cpu_probe.arm_copy_first_row();
    }

    /// Выключает пробу. Безопасно вызывать сколько угодно раз.
    pub fn disarm_cpu_probe(&self) {
        self.cpu_probe.disarm();
    }

    pub fn cpu_probe_mode(&self) -> CpuProbeMode {
        self.cpu_probe.mode()
    }

    /// Свежее смещение `BOOTTIME - MONOTONIC` для перевода моментов показа в шкалу сенсора.
    pub fn sample_clock_offset(&self) -> Result<ClockOffset, ClockReadError> {
        self.clocks.sample_offset()
    }
}

#[cfg(test)]
#[path = "raw_stream_services_tests.rs"]
mod tests;
