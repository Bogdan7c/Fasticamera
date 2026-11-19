//! Ряд отсчётов сценария `ae-trace`: что записывается на каждом шаге и как сырые показания превращаются в отсчёт.
//!
//! Все уровни света — в EV (log2) от «выдержка × ISO» в секундах·ISO, как у диагностики нашего AE:
//! * `observed_ev` — сырой замер сцены (без сглаживания),
//! * `target_ev` — сглаженная цель контроллера,
//! * `sensor_ev` — что сенсор реально снял: выдержка и ISO последнего результата повторяющегося запроса,
//! * `gain_ev` — остаточная подтяжка ISP; на экране уровень `sensor_ev + gain_ev`.
//!
//! Модуль только записывает. Разбор ряда — `ae_trace_analysis`, отчёт — `ae_trace_report`.

use std::time::Duration;

use camera_contract::auto_exposure::ProposalConvergence;
use camera_contract::three_a::{RequestOrigin, SceneFlicker, ThreeAProfile};
use camera_core::three_a_status::LastThreeAResult;
use render_loop::viewfinder::AutoExposureDiagnostics;

/// Ответ детектора мерцания HAL на кадре. «Ключа нет» и «детектор сказал `None`» — разные ответы.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FlickerReading {
    /// HAL ключ `sceneFlicker` в результате не отдал.
    Absent,
    /// Детектор отдал «мерцания нет».
    None,
    /// Детектор отдал 50 Гц.
    Mains50Hz,
    /// Детектор отдал 60 Гц.
    Mains60Hz,
}

impl FlickerReading {
    /// Все ответы в порядке кодов.
    pub const ALL: [FlickerReading; 4] = [
        FlickerReading::Absent,
        FlickerReading::None,
        FlickerReading::Mains50Hz,
        FlickerReading::Mains60Hz,
    ];

    /// Из того, что лежит в результате HAL (`None` снаружи — ключа нет).
    pub fn of_hal(reading: Option<SceneFlicker>) -> Self {
        match reading {
            None => FlickerReading::Absent,
            Some(SceneFlicker::None) => FlickerReading::None,
            Some(SceneFlicker::Mains50Hz) => FlickerReading::Mains50Hz,
            Some(SceneFlicker::Mains60Hz) => FlickerReading::Mains60Hz,
        }
    }

    /// Имя для отчёта.
    pub fn name(self) -> &'static str {
        match self {
            FlickerReading::Absent => "absent",
            FlickerReading::None => "none",
            FlickerReading::Mains50Hz => "mains_50hz",
            FlickerReading::Mains60Hz => "mains_60hz",
        }
    }

    /// Короткий код для строк ряда (расшифровка — в легенде подробностей).
    pub fn code(self) -> u8 {
        match self {
            FlickerReading::Absent => 0,
            FlickerReading::None => 1,
            FlickerReading::Mains50Hz => 2,
            FlickerReading::Mains60Hz => 3,
        }
    }
}

/// Короткий код сходимости предложения для строк ряда: 0 — предложений ещё не было.
pub fn convergence_code(convergence: Option<ProposalConvergence>) -> u8 {
    match convergence {
        None => 0,
        Some(ProposalConvergence::Searching) => 1,
        Some(ProposalConvergence::Settled) => 2,
        Some(ProposalConvergence::Holding) => 3,
    }
}

/// Кадр повторяющегося запроса, впервые увиденный на этом шаге.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NewFrame {
    /// Выдержка кадра, мс (`None` — HAL не отдал).
    pub exposure_ms: Option<f64>,
    pub flicker: FlickerReading,
}

/// Один отсчёт ряда.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TraceSample {
    /// Время от начала расписания, мс.
    pub t_ms: u64,
    pub observed_ev: Option<f64>,
    pub target_ev: Option<f64>,
    pub sensor_ev: Option<f64>,
    pub gain_ev: f64,
    pub convergence: Option<ProposalConvergence>,
    /// Накопительный счётчик предложений нашего AE, опубликованных движку.
    pub proposals_published: u64,
    /// Поколение применённого указания движка (растёт на каждой перестройке запроса).
    pub directive_generation: Option<u64>,
    /// Профиль 3A, который движок держал на этом шаге.
    pub profile: ThreeAProfile,
    /// Новый кадр повторяющегося запроса (по номеру кадра); `None` — нового не было.
    pub new_frame: Option<NewFrame>,
    /// Последний ответ детектора мерцания (держится между кадрами); `None` — кадров ещё не было.
    pub latest_flicker: Option<FlickerReading>,
}

impl TraceSample {
    /// Уровень на экране: снятый сенсором плюс подтяжка ISP.
    pub fn screen_ev(&self) -> Option<f64> {
        self.sensor_ev.map(|sensor| sensor + self.gain_ev)
    }
}

/// log2 от «выдержка × ISO» в секундах·ISO; `None`, если произведение нулевое.
pub fn product_ev(exposure: Duration, iso: u32) -> Option<f64> {
    let seconds_iso = exposure.as_secs_f64() * f64::from(iso);
    (seconds_iso > 0.0).then(|| seconds_iso.log2())
}

/// Превращает показания тика в отсчёт и помнит, какой кадр уже учтён.
#[derive(Debug, Default)]
pub struct SampleTaker {
    last_frame_number: Option<i64>,
    /// Уровень последнего результата повторяющегося запроса: между кадрами отсчёты держат его.
    latest_sensor_ev: Option<f64>,
    /// Последний ответ детектора мерцания.
    latest_flicker: Option<FlickerReading>,
}

impl SampleTaker {
    /// Отсчёт по диагностике AE и последнему результату HAL. Результат не повторяющегося запроса
    /// (тап, серия) в ряд не идёт: у него своя экспозиция.
    pub fn take(
        &mut self,
        t_ms: u64,
        diagnostics: &AutoExposureDiagnostics,
        last_result: Option<&LastThreeAResult>,
        profile: ThreeAProfile,
    ) -> TraceSample {
        let new_frame = last_result
            .filter(|result| result.origin == RequestOrigin::Repeating)
            .and_then(|result| self.accept_frame(result));
        TraceSample {
            t_ms,
            observed_ev: diagnostics.observed_scene_log2_seconds_iso,
            target_ev: diagnostics.target_log2_seconds_iso,
            sensor_ev: self.latest_sensor_ev,
            gain_ev: f64::from(diagnostics.display_gain_ev),
            convergence: diagnostics.convergence,
            proposals_published: diagnostics.counters.proposals_published,
            directive_generation: diagnostics.directive_generation,
            profile,
            new_frame,
            latest_flicker: self.latest_flicker,
        }
    }

    /// Новый кадр повторяющегося запроса (по номеру) обновляет уровень сенсора; повтор — нет.
    fn accept_frame(&mut self, result: &LastThreeAResult) -> Option<NewFrame> {
        if self.last_frame_number == Some(result.frame_number.0) {
            return None;
        }
        self.last_frame_number = Some(result.frame_number.0);
        self.latest_sensor_ev = result
            .exposure_time
            .zip(result.sensitivity_iso)
            .and_then(|(exposure, iso)| product_ev(exposure, iso));
        let flicker = FlickerReading::of_hal(result.scene_flicker);
        self.latest_flicker = Some(flicker);
        Some(NewFrame {
            exposure_ms: result.exposure_time.map(|exposure| exposure.as_secs_f64() * 1_000.0),
            flicker,
        })
    }
}

/// Что случилось с отсчётом при записи.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    Stored,
    /// Ряд полон: отсчёт отброшен и посчитан (конфиг проверяет, что расписание влезает, так что это поломка).
    DroppedFull,
}

/// Ряд с пределом ёмкости: долгий прогон не растит память.
#[derive(Debug)]
pub struct SeriesBuffer {
    samples: Vec<TraceSample>,
    capacity: usize,
    dropped: u64,
}

impl SeriesBuffer {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            samples: Vec::with_capacity(capacity.min(MAX_PREALLOCATED_SAMPLES)),
            capacity,
            dropped: 0,
        }
    }

    pub fn push(&mut self, sample: TraceSample) -> PushOutcome {
        if self.samples.len() >= self.capacity {
            self.dropped += 1;
            return PushOutcome::DroppedFull;
        }
        self.samples.push(sample);
        PushOutcome::Stored
    }

    /// Отсчёты по возрастанию времени.
    pub fn samples(&self) -> &[TraceSample] {
        &self.samples
    }

    /// Сколько отсчётов отброшено из-за полного ряда.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

/// Больше этого заранее не резервируем: ёмкость из конфига может быть произвольной.
const MAX_PREALLOCATED_SAMPLES: usize = 8_192;

#[cfg(test)]
#[path = "ae_trace_series_tests.rs"]
mod tests;
