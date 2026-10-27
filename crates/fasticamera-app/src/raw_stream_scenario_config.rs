//! Параметры сценариев `raw-stream`, `raw-stream-endurance` и `raw-stream-cpu-probe`
//! (`camera.scenarios.raw_stream`, сессия 05).
//!
//! Все числа, от которых зависит вердикт (сколько длится замер, какая доля цели считается успехом,
//! какая доля кадров должна дойти до экрана), живут здесь, а не в коде сценария: сценарий только
//! исполняет таблицу. Типы настроек (`RawFormatSetting`, `ReadoutSetting`, `LensRoleSetting`)
//! общие с остальными сценариями камеры (`camera_scenario_config`).

use camera_contract::ids::CameraTarget;
use camera_contract::stream::{InvalidFrameRate, StreamMode, TargetFrameRate};
use serde::{Deserialize, Serialize};

use crate::camera_scenario_config::{LensRoleSetting, RawFormatSetting, ReadoutSetting, ResolvedStep};

/// Какой из трёх прогонов потока кадров идёт.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawStreamRunKind {
    /// `raw-stream`: минутный замер всей цепочки «камера -> лизинг -> рендер -> экран».
    Standard,
    /// `raw-stream-endurance`: то же, но на пять минут — критерий выхода сессии 05.
    Endurance,
    /// `raw-stream-cpu-probe` (только отладка): доказательство, что счётчик CPU-копий живой.
    CpuProbe,
}

/// Параметры сценариев потока кадров.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RawStreamScenarioConfig {
    /// Какую камеру открывать по роли. Основная (`wide`, камера 2) — физическая: у неё HAL шлёт
    /// финальные результаты кадров, поэтому спаривание кадра с метаданными проверяемо.
    pub role: LensRoleSetting,
    /// Формат RAW. RAW10 — рабочий формат продукта (решение 21).
    pub raw_format: RawFormatSetting,
    /// Какую часть сенсора читать. 4:3 — полный кадр сенсора, как в продукте.
    pub readout: ReadoutSetting,
    /// Целевая частота, к/с. 60 — цель сессии 05 (4K60 RAW, решение 36).
    pub frame_rate: u32,
    /// Длина замера `raw-stream`, мс. Минута — 3600 кадров: достаточно, чтобы увидеть утечку
    /// по одному буферу в секунду и дрейф счётчиков, и быстро для ручных прогонов.
    pub measure_ms: u64,
    /// Длина замера `raw-stream-endurance`, мс. Пять минут — критерий выхода сессии
    /// (`user/foundation/05-zero-copy-raw-stream.md`): за это время успевают проявиться утечки и
    /// первые тепловые эффекты.
    pub endurance_measure_ms: u64,
    /// Длина замера `raw-stream-cpu-probe`, мс. Проба копирует кадры на каждом кадре, 5 с (300
    /// кадров) с большим запасом хватает, чтобы счётчик вырос.
    pub cpu_probe_measure_ms: u64,
    /// Сколько ждать `Streaming` после открытия, мс. Первый запуск сессии на телефоне занимает
    /// секунды (HAL); 15 с — как у `camera-stream`.
    pub streaming_timeout_ms: u64,
    /// Сколько ждать, пока рендер покажет первый кадр камеры после `Streaming`, мс. Рендеру нужны
    /// surface на переднем плане и первый импорт буфера в Vulkan.
    pub render_live_timeout_ms: u64,
    /// Сколько ждать после первого показанного кадра камеры до начала замера, мс. Сразу после
    /// старта потока автоэкспозиция HAL ещё не сошлась: первые кадры почти чёрные (телефон,
    /// сессия 05: яркость скриншота 0,7 из 255 на первом кадре и ~70–120 через 1,5 с). Замер
    /// и скриншоты должны видеть установившийся поток. 0 — начинать сразу.
    pub viewfinder_settle_ms: u64,
    /// Как часто сценарий пишет стадию `progress` во время замера, мс: по ней видно, что длинный
    /// прогон жив и в какую сторону ползут счётчики. 10 с — ~30 строк за пять минут.
    pub progress_interval_ms: u64,
    /// Доля цели, ниже которой частота по меткам сенсора считается провалом. 0,9: средняя частота
    /// не бывает ровно 60,00 (джиттер, первые кадры), а провал 60 -> 30 виден с огромным запасом.
    pub min_fraction_of_target: f64,
    /// Доля кадров, которые обязаны дойти до экрана: `показано >= доля * длительность *
    /// min(частота экрана, частота камеры)`. 0,9 допускает редкие пропуски показа (пейсинг судит
    /// их строже отдельно в сценарии `render`).
    pub min_presented_fraction: f64,
    /// Доля кадров с метаданными: `спарено / (спарено + ушло без пары)`. 0,99 — решение 42:
    /// кадр без пары допустим только как редкое исключение.
    pub min_paired_fraction: f64,
    /// Проверять ли спаривание. `true` для физических камер; логическая камера 0 финальных
    /// результатов не шлёт (факт сессии 04), для неё ставят `false`.
    pub expect_paired_metadata: bool,
    /// Сколько замеров задержки «сенсор -> дисплей» хранить. 40 000 — ~11 минут при 60 к/с;
    /// сверх лимита замеры считаются потерянными (счётчик в отчёте), а не растут без предела.
    pub latency_samples_limit: usize,
    /// Задержка выше этого порога (мс) считается ошибкой сопоставления (чужой `present_id`), а не
    /// измерением: в нормальной цепочке видоискателя задержка — десятки миллисекунд.
    pub latency_plausible_max_ms: u64,
}

impl Default for RawStreamScenarioConfig {
    fn default() -> Self {
        Self {
            role: LensRoleSetting::Wide,
            raw_format: RawFormatSetting::Raw10,
            readout: ReadoutSetting::FourByThree,
            frame_rate: 60,
            measure_ms: 60_000,
            endurance_measure_ms: 300_000,
            cpu_probe_measure_ms: 5_000,
            streaming_timeout_ms: 15_000,
            render_live_timeout_ms: 15_000,
            viewfinder_settle_ms: 1_500,
            progress_interval_ms: 10_000,
            min_fraction_of_target: 0.9,
            min_presented_fraction: 0.9,
            min_paired_fraction: 0.99,
            expect_paired_metadata: true,
            latency_samples_limit: 40_000,
            latency_plausible_max_ms: 2_000,
        }
    }
}

/// Неверные параметры сценариев потока кадров.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RawStreamScenarioConfigError {
    #[error("{field} должно быть больше нуля")]
    Zero { field: &'static str },
    #[error("{field} = {value}: доля должна быть конечной, больше нуля и не больше 1")]
    FractionOutOfRange { field: &'static str, value: f64 },
    #[error("частота: {0}")]
    InvalidFrameRate(#[from] InvalidFrameRate),
}

impl RawStreamScenarioConfig {
    /// Длительность замера выбранного прогона, мс.
    pub fn measure_ms(&self, kind: RawStreamRunKind) -> u64 {
        match kind {
            RawStreamRunKind::Standard => self.measure_ms,
            RawStreamRunKind::Endurance => self.endurance_measure_ms,
            RawStreamRunKind::CpuProbe => self.cpu_probe_measure_ms,
        }
    }

    /// Цель и режим потока сценария.
    pub fn resolve(&self) -> Result<ResolvedStep, RawStreamScenarioConfigError> {
        Ok(ResolvedStep {
            target: CameraTarget::Role(self.role.to_contract()),
            mode: StreamMode {
                raw_format: self.raw_format.to_contract(),
                readout: self.readout.to_contract(),
                frame_rate: TargetFrameRate::new(self.frame_rate)?,
            },
        })
    }

    pub fn validate(&self) -> Result<(), RawStreamScenarioConfigError> {
        for (field, value) in [
            ("raw_stream.measure_ms", self.measure_ms),
            ("raw_stream.endurance_measure_ms", self.endurance_measure_ms),
            ("raw_stream.cpu_probe_measure_ms", self.cpu_probe_measure_ms),
            ("raw_stream.streaming_timeout_ms", self.streaming_timeout_ms),
            ("raw_stream.render_live_timeout_ms", self.render_live_timeout_ms),
            ("raw_stream.progress_interval_ms", self.progress_interval_ms),
            ("raw_stream.latency_plausible_max_ms", self.latency_plausible_max_ms),
        ] {
            if value == 0 {
                return Err(RawStreamScenarioConfigError::Zero { field });
            }
        }
        if self.latency_samples_limit == 0 {
            return Err(RawStreamScenarioConfigError::Zero {
                field: "raw_stream.latency_samples_limit",
            });
        }
        for (field, value) in [
            ("raw_stream.min_fraction_of_target", self.min_fraction_of_target),
            ("raw_stream.min_presented_fraction", self.min_presented_fraction),
            ("raw_stream.min_paired_fraction", self.min_paired_fraction),
        ] {
            // Через contains: NaN не лежит ни в каком диапазоне и отвергается.
            if !(value > 0.0 && value <= 1.0) {
                return Err(RawStreamScenarioConfigError::FractionOutOfRange { field, value });
            }
        }
        self.resolve()?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "raw_stream_scenario_config_tests.rs"]
mod tests;
