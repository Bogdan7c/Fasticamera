//! Синтетические сцены видоискателя и общий паритет «GPU против эталона» (все основы тона).
//!
//! Сама линейка (допуски, числа сверки, сцены, прогон GPU и эталона) живёт в библиотеке
//! `isp_vulkan::saturation::parity`: её же гоняет приложение на телефоне. Здесь — тонкая обвязка под `ParamsSpec`
//! (калибровка тестов) и сообщения тестов.

use isp_vulkan::saturation::parity::{self, ParityRuler, SceneCalibration};

use super::*;

#[allow(unused_imports, reason = "каждый тестовый бинарь использует свою часть помощников")]
pub use isp_vulkan::saturation::parity::{
    ErrorAllowance, HALF_LSB, HEIGHT_OF_SYNTHETIC_SCENES, MATH_EXCESS_TOLERANCE, ParityMeasure,
    WIDTH_OF_SYNTHETIC_SCENES,
};

impl ParamsSpec {
    /// Калибровка кадра для библиотечных сцен: раскладка, уровни, нейтраль.
    pub fn scene_calibration(&self) -> SceneCalibration {
        SceneCalibration {
            pattern: self.pattern,
            black: self.black,
            white: self.white,
            neutral: self.neutral,
        }
    }
}

/// Четыре базовые сцены синтетики: имя и кадр (серая шкала — первая).
pub fn synthetic_scenes(spec: &ParamsSpec) -> Vec<(&'static str, RawFrame)> {
    parity::synthetic_scenes(&spec.scene_calibration())
}

/// Развёртка по оттенку (худший случай полос оттенка).
pub fn hue_sweep(spec: &ParamsSpec) -> RawFrame {
    parity::hue_sweep_scene(&spec.scene_calibration())
}

/// Кожа и малоцветные цвета рядом.
pub fn skin_and_pastels(spec: &ParamsSpec) -> RawFrame {
    parity::skin_and_pastels_scene(&spec.scene_calibration())
}

/// Яркие насыщенные цвета (худший случай ручек тона).
pub fn bright_saturated(spec: &ParamsSpec) -> RawFrame {
    parity::bright_saturated_scene(&spec.scene_calibration())
}

/// Сцены паритета вида: четыре базовые, развёртка по оттенку, кожа и пастели.
pub fn look_scenes(spec: &ParamsSpec) -> Vec<(&'static str, RawFrame)> {
    parity::look_scenes(&spec.scene_calibration())
}

/// Сцены паритета тона: серая шкала, яркие насыщенные, развёртка по оттенку.
pub fn tone_scenes(spec: &ParamsSpec) -> Vec<(&'static str, RawFrame)> {
    parity::tone_scenes(&spec.scene_calibration())
}

/// Прогоняет кадр в обоих форматах и сравнивает с эталоном; допусков не проверяет (см. [`ParityMeasure::violations`]).
pub fn measure_viewfinder_parity(
    context: &GpuContext,
    raw: &RawFrame,
    params: &IspParams,
    spec: &ParamsSpec,
) -> ParityMeasure {
    ParityRuler::new(context)
        .expect("линейка паритета создаётся")
        .measure(raw, params, spec.display())
        .expect("линейка паритета измерила кадр")
}

/// Прогоняет кадр в обоих форматах и проверяет действующий допуск; возвращает (fp16, 10 бит) max |Δ|.
pub fn assert_viewfinder_parity(
    context: &GpuContext,
    name: &str,
    raw: &RawFrame,
    params: &IspParams,
    spec: &ParamsSpec,
) -> (f32, f32) {
    assert_viewfinder_parity_allowing(context, name, raw, params, spec, ErrorAllowance::Standard)
}

/// То же с явным допуском случая ([`ErrorAllowance`]).
pub fn assert_viewfinder_parity_allowing(
    context: &GpuContext,
    name: &str,
    raw: &RawFrame,
    params: &IspParams,
    spec: &ParamsSpec,
    allowance: ErrorAllowance,
) -> (f32, f32) {
    let measure = measure_viewfinder_parity(context, raw, params, spec);
    for (label, parity) in [("fp16", &measure.half), ("10 бит", &measure.ten_bit)] {
        eprintln!(
            "{name} {label}: max |Δ| = {:.3e} ({:.3} LSB), доля > 1 LSB = {:.4}, сверх шага хранения = {:.2e}",
            parity.max_abs,
            parity.max_abs / ONE_LSB,
            parity.over_one_lsb,
            parity.math_excess
        );
    }
    let violations = measure.violations(allowance);
    assert!(violations.is_empty(), "{name}: {violations:?}");
    (measure.half.max_abs, measure.ten_bit.max_abs)
}
