//! Итог линейки паритета в строке стенда (сессия 14o): что показать в результате и когда это провал.
//!
//! Линейка (`isp_vulkan::saturation::parity`) отдаёт числа; здесь они сворачиваются в одну запись на работу
//! [`ParityRecord`]: худшая ошибка математики сверх шага хранения, худший max |Δ|, допуск и нарушения. Запись уходит
//! в подробности строки (`FASTICAMERA_DETAIL`) целиком, а в строку результата — одной КОРОТКОЙ строкой
//! [`ParityOutcome::compact_row`] (`имя|ошибка|max_abs|допуск|вердикт`), которую читает `scripts/smoke_checks_saturation.py`.

use gpu_vulkan::storage_image::StorageImageFormat;
use isp_vulkan::saturation::parity::{ErrorAllowance, ParityMeasure};
use serde::Serialize;

/// Вердикт в компактной строке: допуск держится.
pub const VERDICT_PASS: &str = "pass";
/// Вердикт в компактной строке: есть нарушение допуска.
pub const VERDICT_FAIL: &str = "fail";
/// Вердикт в компактной строке: измерить не удалось (числа нет).
pub const VERDICT_UNAVAILABLE: &str = "unavailable";
/// Ячейка компактной строки, когда числа нет.
const NO_NUMBER_CELL: &str = "-";

/// Накопитель худшего по сценам: из него строится [`ParityRecord`].
#[derive(Debug, Clone)]
pub struct ParityAccumulator {
    allowance: ErrorAllowance,
    worst_math_excess: f32,
    worst_max_abs: f32,
    worst_scene: &'static str,
    half_excess: f32,
    ten_bit_excess: f32,
    ten_bit_format: Option<StorageImageFormat>,
    scenes_measured: usize,
    violations: Vec<String>,
}

impl ParityAccumulator {
    /// Пустой накопитель под допуск `allowance`.
    pub fn new(allowance: ErrorAllowance) -> Self {
        Self {
            allowance,
            worst_math_excess: 0.0,
            worst_max_abs: 0.0,
            worst_scene: "none",
            half_excess: 0.0,
            ten_bit_excess: 0.0,
            ten_bit_format: None,
            scenes_measured: 0,
            violations: Vec::new(),
        }
    }

    /// Принимает измерение сцены `scene`. Формат хранения «10 бит» судится как на устройстве: запасной fp16 — не
    /// нарушение (его число уже честно посчитано под шагом fp16), поэтому берутся только числовые нарушения.
    pub fn add_scene(&mut self, scene: &'static str, measure: &ParityMeasure) {
        if measure.worst_math_excess() >= self.worst_math_excess {
            self.worst_math_excess = measure.worst_math_excess();
            self.worst_scene = scene;
        }
        self.worst_max_abs = self.worst_max_abs.max(measure.worst_max_abs());
        self.half_excess = self.half_excess.max(measure.half.math_excess);
        self.ten_bit_excess = self.ten_bit_excess.max(measure.ten_bit.math_excess);
        self.ten_bit_format = Some(measure.ten_bit_format);
        self.scenes_measured += 1;
        self.violations.extend(
            measure
                .math_violations(self.allowance)
                .into_iter()
                .map(|violation| format!("{scene}: {violation}")),
        );
    }

    /// Итоговая запись; без единой сцены — [`ParityOutcome::Unavailable`] (пустое измерение не бывает «прошло»).
    pub fn finish(self) -> ParityOutcome {
        match self.ten_bit_format {
            None => ParityOutcome::Unavailable {
                reason: "ни одна сцена не измерена".to_owned(),
            },
            Some(format) => ParityOutcome::Measured(ParityRecord {
                allowance: self.allowance.name(),
                tolerance: self.allowance.math_excess(),
                worst_math_excess: self.worst_math_excess,
                worst_max_abs: self.worst_max_abs,
                worst_scene: self.worst_scene,
                scenes_measured: self.scenes_measured,
                fp16_math_excess: self.half_excess,
                preferred_math_excess: self.ten_bit_excess,
                preferred_format: format_name(format),
                violations: self.violations,
            }),
        }
    }
}

/// Стабильное имя формата хранения для отчёта.
fn format_name(format: StorageImageFormat) -> &'static str {
    match format {
        StorageImageFormat::Rgba16Float => "rgba16f",
        StorageImageFormat::A2B10G10R10Unorm => "rgb10a2",
    }
}

/// Измеренный паритет работы.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParityRecord {
    /// Имя допуска (`ErrorAllowance::name`).
    pub allowance: &'static str,
    /// Допуск ошибки математики сверх шага хранения.
    pub tolerance: f32,
    /// Худшая ошибка математики сверх шага хранения по сценам и форматам (число «ошибка математики»).
    pub worst_math_excess: f32,
    /// Худший max |Δ| по сценам и форматам.
    pub worst_max_abs: f32,
    /// Сцена, на которой достигнута худшая ошибка математики.
    pub worst_scene: &'static str,
    /// Сколько сцен измерено.
    pub scenes_measured: usize,
    /// Худшая ошибка математики в формате fp16.
    pub fp16_math_excess: f32,
    /// Худшая ошибка математики в формате, который просит продукт.
    pub preferred_math_excess: f32,
    /// Формат, который устройство дало на запрос продукта (`rgb10a2` или запасной `rgba16f`).
    pub preferred_format: &'static str,
    /// Нарушения допуска (пусто — паритет держится).
    pub violations: Vec<String>,
}

impl ParityRecord {
    /// Допуск держится: нет ни одного нарушения И худшая ошибка не больше допуска.
    pub fn passed(&self) -> bool {
        self.violations.is_empty() && self.worst_math_excess <= self.tolerance
    }
}

/// Исход измерения паритета работы.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ParityOutcome {
    /// Числа получены.
    Measured(ParityRecord),
    /// Измерить не удалось (ошибка GPU, параметров, эталона): числа нет, и это провал, а не «прошло».
    Unavailable { reason: String },
}

impl ParityOutcome {
    /// Нарушение для вердикта строки: `None`, если паритет держится.
    pub fn violation(&self) -> Option<String> {
        match self {
            Self::Measured(record) if record.passed() => None,
            Self::Measured(record) => Some(format!(
                "ошибка математики {:.2e} при допуске {:.0e} (сцена {}): {}",
                record.worst_math_excess,
                record.tolerance,
                record.worst_scene,
                record.violations.join("; ")
            )),
            Self::Unavailable { reason } => Some(format!("паритет не измерен: {reason}")),
        }
    }

    /// Короткая строка результата: `имя|ошибка|max_abs|допуск|вердикт` (контракт с `smoke_checks_saturation.py`).
    pub fn compact_row(&self, name: &str) -> String {
        match self {
            Self::Measured(record) => {
                let verdict = if record.passed() { VERDICT_PASS } else { VERDICT_FAIL };
                format!(
                    "{name}|{:.2e}|{:.2e}|{:.0e}|{verdict}",
                    record.worst_math_excess, record.worst_max_abs, record.tolerance
                )
            }
            Self::Unavailable { .. } => {
                format!("{name}|{NO_NUMBER_CELL}|{NO_NUMBER_CELL}|{NO_NUMBER_CELL}|{VERDICT_UNAVAILABLE}")
            }
        }
    }
}

#[cfg(test)]
#[path = "parity_report_tests.rs"]
mod tests;
