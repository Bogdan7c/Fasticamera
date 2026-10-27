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
use raw_frame_stream::{PoolPlan, RawFrameStream, RawFrameStreamConfig, RawStreamCounters};

use crate::clock_offset::{ClockOffset, ClockOffsetSource, ClockReadError, SystemClocks};

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
}

impl RawStreamServices {
    pub fn new(
        counters: Arc<dyn RawStreamCounterSource>,
        cpu_probe: CpuAccessProbe,
        clocks: Arc<dyn ClockOffsetSource>,
        stream_config: RawFrameStreamConfig,
    ) -> Self {
        Self {
            counters,
            cpu_probe,
            clocks,
            stream_config,
        }
    }

    /// Сервис без живого потока: счётчики нулевые, проба выключена и никому не подключена.
    /// Для сборок без камеры и тестов, которым поток не нужен.
    pub fn detached() -> Self {
        Self::new(
            Arc::new(NoStreamCounters),
            CpuAccessProbe::disarmed(),
            Arc::new(SystemClocks),
            RawFrameStreamConfig::default(),
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
            viewfinder: 1 + config.viewfinder_render_holds,
            pending_pairs: config.max_pending_frames,
            snapshot_ring: if config.snapshot_ring_budget_bytes == 0 {
                0
            } else {
                config.snapshot_ring_max_frames
            },
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
