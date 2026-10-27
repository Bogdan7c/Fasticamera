//! Длинный прогон видоискателя (план 07, «10 минут с настоящим ISP»): снимок раз в минуту.
//!
//! Каждая минута — окно замера тока без разогрева ([`PowerWindowAccumulator`]): средний ток,
//! расход заряда по `CHARGE_COUNTER`, температура батареи, дыры. К нему сценарий добавляет то, что
//! знает только он: тепловой статус и запас ([`ThermalSnapshot`]), частоту по `SENSOR_TIMESTAMP` и
//! время прохода ISP на GPU ([`MinuteObservation`]; последние два — открытые места, пока их
//! заполняет `None`). Минута на внешнем питании не останавливает прогон (тепло и частота всё равно
//! нужны), но её ток помечается отказом, а в итоговые токи не входит.
//!
//! Как сценарий ведёт прогон: в начале — [`EnduranceRecorder::start`] со снимком тепла; на каждом
//! тике — [`EnduranceRecorder::on_tick`] с показаниями батареи и замыканием, которое снимает
//! наблюдение минуты (вызывается только когда минута закрылась). [`EnduranceStep::Finished`] —
//! прогон окончен; [`EnduranceRecorder::report`] → `summary_within` для строки результата и
//! `detail_items` для строк `detail_*`.

use serde::Serialize;
use serde_json::Value;
use shell_contract::clock::MonotonicNanos;
use shell_contract::power::{BatterySample, DeciCelsius, DischargeSign, DischargeSignEvidence, detect_discharge_sign};
use shell_contract::thermal::{HeadroomForecast, ThermalHeadroom, ThermalSource, ThermalStatus};

use super::config::{PowerSettings, SignPolicy};
use super::window::{PowerWindowAccumulator, WindowMeasurement, WindowPush, WindowRejection, WindowSpan, offset};

/// Тепло в один момент.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ThermalSnapshot {
    /// Статус троттлинга; `None` — не прочитался (причина в `error`).
    pub status: Option<ThermalStatus>,
    /// Запас до троттлинга; `None` — система не дала значения.
    pub headroom: Option<f32>,
    pub error: Option<String>,
}

impl ThermalSnapshot {
    /// Читает источник тепла (сервис оболочки; сам по себе не блокирует).
    pub fn read(source: &dyn ThermalSource) -> Self {
        let (status, error) = match source.current_status() {
            Ok(status) => (Some(status), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let headroom = match source.headroom(HeadroomForecast::NOW) {
            ThermalHeadroom::Measured(value) => Some(value),
            ThermalHeadroom::Unavailable => None,
        };
        Self {
            status,
            headroom,
            error,
        }
    }
}

/// Время прохода ISP на GPU за минуту (заполнит сценарий, когда появятся timestamp-запросы).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct IspGpuTime {
    pub median_ms: f64,
    pub p95_ms: f64,
}

/// То, что сценарий наблюдает сам в конце минуты.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MinuteObservation {
    pub thermal: ThermalSnapshot,
    /// Частота по `SENSOR_TIMESTAMP` за минуту; `None` — пока не заполняется.
    pub sensor_fps: Option<f64>,
    /// Время ISP на GPU; `None` — пока не заполняется.
    pub isp_gpu_time: Option<IspGpuTime>,
}

/// Ток минуты.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(tag = "power", rename_all = "snake_case")]
pub enum MinutePower {
    Measured(WindowMeasurement),
    Rejected {
        rejection: WindowRejection,
    },
    /// Знак тока ещё не установлен (или противоречит установленному) — ток не считается.
    SignUndetermined {
        evidence: DischargeSignEvidence,
    },
}

/// Снимок одной минуты.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EnduranceSnapshot {
    /// Номер минуты с 1.
    pub minute: u32,
    pub power: MinutePower,
    pub observation: MinuteObservation,
}

/// Итоги прогона по годным минутам.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct EnduranceTotals {
    pub measured_minutes: u32,
    pub rejected_minutes: u32,
    /// Среднее средних токов годных минут, мА (минуты равной длины — это среднее за прогон).
    pub mean_discharge_ma: Option<f64>,
    /// Сумма расхода заряда годных минут, мА·ч (`None` — ни в одной минуте нет счётчика).
    pub charge_used_mah: Option<f64>,
    pub battery_temperature_start: Option<DeciCelsius>,
    pub battery_temperature_end: Option<DeciCelsius>,
    /// Худший тепловой статус за прогон (начало и все минуты).
    pub worst_thermal_status: Option<ThermalStatus>,
    /// Наибольший headroom за прогон. Шкала Android растёт к троттлингу: 0 — далеко, 1,0 — порог
    /// тяжёлого (`SEVERE`), поэтому худший момент — максимум.
    pub peak_headroom: Option<f32>,
}

/// Отчёт прогона.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EnduranceReport {
    pub discharge_sign: Option<DischargeSign>,
    pub start: ThermalSnapshot,
    pub totals: EnduranceTotals,
    pub minutes: Vec<EnduranceSnapshot>,
    /// Показания раньше начала прогона (лежали в кольце до старта) — отброшены.
    pub samples_before_start: u64,
}

/// Что дальше.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnduranceStep {
    /// Идёт минута `minute` (с 1).
    Recording {
        minute: u32,
    },
    Finished,
}

/// Запись длинного прогона.
pub struct EnduranceRecorder {
    settings: PowerSettings,
    started_at: MonotonicNanos,
    start: ThermalSnapshot,
    /// Открытая минута (номер с 1) и её накопитель; `None` — прогон окончен.
    open: Option<(u32, PowerWindowAccumulator)>,
    sign: Option<DischargeSign>,
    minutes: Vec<EnduranceSnapshot>,
    samples_before_start: u64,
}

impl EnduranceRecorder {
    /// Прогон с момента `started_at`; `start` — тепло в начале.
    pub fn start(settings: &PowerSettings, started_at: MonotonicNanos, start: ThermalSnapshot) -> Self {
        let sign = match settings.sign {
            SignPolicy::Fixed(sign) => Some(sign),
            SignPolicy::Detect { .. } => None,
        };
        let mut recorder = Self {
            settings: *settings,
            started_at,
            start,
            open: None,
            sign,
            minutes: Vec::new(),
            samples_before_start: 0,
        };
        recorder.open = Some((1, recorder.minute_accumulator(1)));
        recorder
    }

    /// Сколько минут (снимков) в прогоне: длительность / шаг, с округлением вверх.
    pub fn total_minutes(&self) -> u32 {
        let duration = self.settings.endurance.duration.as_nanos();
        let interval = self.settings.endurance.snapshot_interval.as_nanos().max(1);
        u32::try_from(duration.div_ceil(interval)).unwrap_or(u32::MAX)
    }

    /// Тик: показания батареи и текущий момент. `observe` вызывается по разу на каждую закрытую минуту.
    pub fn on_tick(
        &mut self,
        now: MonotonicNanos,
        samples: &[BatterySample],
        mut observe: impl FnMut() -> MinuteObservation,
    ) -> EnduranceStep {
        for sample in samples {
            self.route(*sample, &mut observe);
        }
        while let Some((_, accumulator)) = self.open.as_ref() {
            if now < accumulator.span().end() {
                break;
            }
            self.close_open_minute(&mut observe);
        }
        match &self.open {
            Some((minute, _)) => EnduranceStep::Recording { minute: *minute },
            None => EnduranceStep::Finished,
        }
    }

    /// Отчёт (в любой момент; до конца — по закрытым минутам).
    pub fn report(&self) -> EnduranceReport {
        EnduranceReport {
            discharge_sign: self.sign,
            start: self.start.clone(),
            totals: self.totals(),
            minutes: self.minutes.clone(),
            samples_before_start: self.samples_before_start,
        }
    }

    fn minute_accumulator(&self, minute: u32) -> PowerWindowAccumulator {
        let interval = self.settings.endurance.snapshot_interval;
        let start = offset(self.started_at, interval.saturating_mul(minute.saturating_sub(1)));
        let end = offset(self.started_at, self.settings.endurance.duration);
        // Последняя минута обрезается концом прогона, если длительность не кратна шагу.
        let length_nanos = end.as_nanos().saturating_sub(start.as_nanos()).max(0);
        let length = interval.min(std::time::Duration::from_nanos(
            u64::try_from(length_nanos).unwrap_or(0),
        ));
        PowerWindowAccumulator::new(
            WindowSpan::new(start, std::time::Duration::ZERO, length),
            self.settings.sampling.period,
        )
    }

    fn route(&mut self, sample: BatterySample, observe: &mut impl FnMut() -> MinuteObservation) {
        while let Some((_, accumulator)) = self.open.as_ref() {
            if sample.at < accumulator.span().end() {
                break;
            }
            self.close_open_minute(observe);
        }
        let Some((_, accumulator)) = self.open.as_mut() else {
            // Прогон окончен: показание после конца на итоги не влияет.
            return;
        };
        // Минуты до этого показания уже закрыты, поэтому «не из окна» здесь значит «раньше начала
        // прогона». Отказ по питанию накопитель помнит сам и отдаст при закрытии минуты.
        if accumulator.push(sample) == WindowPush::OutsideWindow {
            self.samples_before_start += 1;
        }
    }

    fn close_open_minute(&mut self, observe: &mut impl FnMut() -> MinuteObservation) {
        let Some((minute, accumulator)) = self.open.take() else {
            return;
        };
        let power = self.minute_power(&accumulator);
        self.minutes.push(EnduranceSnapshot {
            minute,
            power,
            observation: observe(),
        });
        if minute < self.total_minutes() {
            self.open = Some((minute + 1, self.minute_accumulator(minute + 1)));
        }
    }

    fn minute_power(&mut self, accumulator: &PowerWindowAccumulator) -> MinutePower {
        if let Err(rejection) = accumulator.check_complete(self.settings.max_missing_fraction) {
            return MinutePower::Rejected { rejection };
        }
        let sign = match (self.settings.sign, self.sign) {
            (SignPolicy::Fixed(sign), _) => sign,
            (SignPolicy::Detect { rule }, established) => {
                match (detect_discharge_sign(accumulator.measured_samples(), rule), established) {
                    (DischargeSignEvidence::Detected { sign, .. }, None) => {
                        self.sign = Some(sign);
                        sign
                    }
                    (DischargeSignEvidence::Detected { sign, .. }, Some(known)) if sign == known => sign,
                    (evidence, _) => return MinutePower::SignUndetermined { evidence },
                }
            }
        };
        match accumulator.finish(sign, self.settings.max_missing_fraction, self.settings.current_range) {
            Ok(measurement) => MinutePower::Measured(measurement),
            Err(rejection) => MinutePower::Rejected { rejection },
        }
    }

    fn totals(&self) -> EnduranceTotals {
        let measured: Vec<&WindowMeasurement> = self
            .minutes
            .iter()
            .filter_map(|snapshot| match &snapshot.power {
                MinutePower::Measured(measurement) => Some(measurement),
                MinutePower::Rejected { .. } | MinutePower::SignUndetermined { .. } => None,
            })
            .collect();
        let measured_minutes = u32::try_from(measured.len()).unwrap_or(u32::MAX);
        let all_minutes = u32::try_from(self.minutes.len()).unwrap_or(u32::MAX);
        let mean_discharge_ma = (!measured.is_empty()).then(|| {
            // Минут единицы — перевод в f64 точен.
            measured.iter().map(|minute| minute.mean_discharge_ma).sum::<f64>() / measured.len() as f64
        });
        let charges: Vec<f64> = measured.iter().filter_map(|minute| minute.charge_used_mah).collect();
        let thermal = std::iter::once(&self.start).chain(self.minutes.iter().map(|minute| &minute.observation.thermal));
        let (worst_thermal_status, peak_headroom) = thermal.fold((None, None), |(worst, top), snapshot| {
            (worst.max(snapshot.status), max_f32(top, snapshot.headroom))
        });
        EnduranceTotals {
            measured_minutes,
            rejected_minutes: all_minutes.saturating_sub(measured_minutes),
            mean_discharge_ma,
            charge_used_mah: (!charges.is_empty()).then(|| charges.iter().sum()),
            battery_temperature_start: measured.iter().find_map(|minute| minute.temperature_start),
            battery_temperature_end: measured.iter().rev().find_map(|minute| minute.temperature_end),
            worst_thermal_status,
            peak_headroom,
        }
    }
}

fn max_f32(left: Option<f32>, right: Option<f32>) -> Option<f32> {
    match (left, right) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (one, None) | (None, one) => one,
    }
}

impl EnduranceReport {
    /// Сводка для строки результата не длиннее `budget_bytes`: целиком, если влезает; иначе —
    /// итоги, тепло в начале и последняя минута, а все минуты уходят строками `detail_*`
    /// ([`EnduranceReport::detail_items`]).
    pub fn summary_within(&self, budget_bytes: usize) -> Value {
        // Сериализация этих типов не падает (ключи — строки); NaN станет `null`, не ошибкой.
        let full = serde_json::to_value(self).unwrap_or(Value::Null);
        let fits = serde_json::to_string(&full).is_ok_and(|json| json.len() <= budget_bytes);
        if fits {
            return full;
        }
        serde_json::json!({
            "minutes_in_detail": true,
            "discharge_sign": self.discharge_sign,
            "start": self.start,
            "totals": self.totals,
            "last_minute": self.minutes.last(),
            "samples_before_start": self.samples_before_start,
        })
    }

    /// Минуты по одной — элементы для `detail_*`.
    pub fn detail_items(&self) -> Vec<Value> {
        self.minutes
            .iter()
            .map(|minute| serde_json::to_value(minute).unwrap_or(Value::Null))
            .collect()
    }
}

#[cfg(test)]
#[path = "endurance_tests.rs"]
mod tests;
