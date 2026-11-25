//! Синтетические сцены видоискателя и общий паритет «GPU против эталона» (обе основы тона).

use super::*;

/// Допуск ошибки вычислений GPU сверх шага хранения (см. модуль).
pub const MATH_EXCESS_TOLERANCE: f32 = 1.0e-5;
/// Сколько ошибки математики допускает случай. Действующий допуск один — [`ErrorAllowance::Standard`]; у двух узких
/// групп случаев свой, с названной причиной: сама формула там раздувает последний разряд входа, и это видно по
/// эталону, а не по шейдеру. Все — много меньше половины ступени 10-битного экрана (4,9e-4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorAllowance {
    /// Допуск 14a.
    Standard,
    /// Прежняя основа, выход sRGB, цвет у края ручек 14l: рабочее пространство узкое, цвет уходит глубоко за охват, и
    /// канал у нуля набирается вычитанием крупных чисел; ≈ 1e-6 линейного света у кода 0,008 (линейный участок OETF,
    /// наклон 12,92) выходят как 1,0…1,6e-5. На прежних, слабых краях те же случаи давали 7e-7.
    LegacySrgbExtremeColour,
    /// Углы тона «контраст −, средние −, света −» разом: наклон кривой у белого ≈ 450, и 1–3 ULP разницы основы на
    /// CPU и GPU становятся 1e-5…1,3e-4 (`look_support::tone_corner_is_ill_conditioned`).
    SteepToneAtWhite,
}

impl ErrorAllowance {
    /// Допуск ошибки математики сверх шага хранения.
    pub fn math_excess(self) -> f32 {
        match self {
            Self::Standard => MATH_EXCESS_TOLERANCE,
            Self::LegacySrgbExtremeColour => 2.0e-5,
            Self::SteepToneAtWhite => 2.0e-4,
        }
    }
}

/// Полшага 10-битного выхода.
pub const HALF_LSB: f32 = 0.5 / 1023.0;

/// Размер кадров синтетики.
pub const WIDTH_OF_SYNTHETIC_SCENES: u32 = 64;
pub const HEIGHT_OF_SYNTHETIC_SCENES: u32 = 48;

/// Сцены синтетики: имя и кадр.
pub fn synthetic_scenes(spec: &ParamsSpec) -> Vec<(&'static str, RawFrame)> {
    let (width, height) = (WIDTH_OF_SYNTHETIC_SCENES, HEIGHT_OF_SYNTHETIC_SCENES);
    let levels = spec.levels();
    let neutral = spec.neutral;
    let brightest = neutral.iter().copied().fold(f64::MIN, f64::max);
    // Серая шкала: серый объект под нейтралью, яркость 0…1,1 слева направо (правый край — пересвет).
    let gray_ramp = RawFrame::from_linear(width, height, spec.pattern, levels, |x, _, channel| {
        f64::from(x) / f64::from(width - 1) * 1.1 * neutral[channel] / brightest
    });
    // Цветные полосы `isp-model` (7 цветов) на уровне 0,6, строки — разная яркость 0,2…1.
    let bars = color_bars(PixelSize::new(width, height), 0.6).unwrap();
    let color_bar_frame = RawFrame::from_linear(width, height, spec.pattern, levels, |x, y, channel| {
        let row_level = 0.2 + 0.8 * f64::from(y) / f64::from(height - 1);
        f64::from(bars.pixel(x, y)[channel]) * row_level / 0.6
    });
    // Пересветы: полностью насыщенные ячейки, насыщен один канал, сильно за белым у красного.
    let highlights = RawFrame::from_linear(width, height, spec.pattern, levels, |x, y, channel| {
        let band = x * 4 / width;
        match (band, channel) {
            (0, _) => 1.0,
            (1, 0) => 1.0,
            (1, _) => 0.4 + 0.01 * f64::from(y),
            (2, 2) => 0.98,
            (2, _) => 0.3,
            _ => 0.9 * neutral[channel] / brightest,
        }
    });
    // Около чёрного: значения от black−3 до black+20 (ниже чёрного обрезаются уровнями).
    let near_black = RawFrame::from_fn(width, height, spec.pattern, |x, y, channel| {
        let base = 61 + ((x + 3 * y + channel as u32) % 24);
        base as u16
    });
    vec![
        ("серая шкала", gray_ramp),
        ("цветные полосы", color_bar_frame),
        ("пересветы", highlights),
        ("около чёрного", near_black),
    ]
}

/// Итог сверки одного кадра с эталоном в обоих форматах выхода: числа, без вердикта.
#[derive(Debug, Clone, Copy)]
pub struct ParityMeasure {
    /// fp16-выход.
    pub half: Parity,
    /// 10-битный выход (RADV пишет rgb10_a2).
    pub ten_bit: Parity,
    /// Формат «10 бит» действительно получен (а не запасной).
    pub ten_bit_format_is_unorm: bool,
}

impl ParityMeasure {
    /// Нарушения допусков паритета (пусто — кадр прошёл): ошибка математики сверх шага хранения не больше
    /// `allowance`, max |Δ| ≤ полшага 10 бит + то же, ноль пикселей хуже 1 LSB.
    pub fn violations(&self, allowance: ErrorAllowance) -> Vec<String> {
        let tolerance = allowance.math_excess();
        let mut found = Vec::new();
        for (label, parity) in [("fp16", &self.half), ("10 бит", &self.ten_bit)] {
            if parity.math_excess > tolerance {
                found.push(format!(
                    "{label}: ошибка математики {:.3e} > {tolerance:e}",
                    parity.math_excess
                ));
            }
            if parity.max_abs > HALF_LSB + tolerance {
                found.push(format!("{label}: max |Δ| {:.3e} > ½ LSB + допуск", parity.max_abs));
            }
        }
        if self.half.over_one_lsb != 0.0 {
            found.push(format!("fp16: доля хуже 1 LSB {}", self.half.over_one_lsb));
        }
        if !self.ten_bit_format_is_unorm {
            found.push("10 бит: формат не rgb10_a2".to_owned());
        }
        found
    }

    /// Наибольшая ошибка математики сверх шага хранения по двум форматам.
    pub fn worst_math_excess(&self) -> f32 {
        self.half.math_excess.max(self.ten_bit.math_excess)
    }
}

/// Прогоняет кадр в обоих форматах и сравнивает с эталоном; допусков не проверяет (см. [`ParityMeasure::violations`]).
pub fn measure_viewfinder_parity(
    context: &GpuContext,
    raw: &RawFrame,
    params: &IspParams,
    spec: &ParamsSpec,
) -> ParityMeasure {
    let request = RunRequest::whole_frame(raw, params, spec.display());
    let reference = reference_upright_encoded(raw, raw.full_rect(), SensorOrientation::Degrees0, params);
    let half = run_viewfinder(context, &test_config(ViewfinderStorageFormat::HalfFloatOnly), &request);
    assert_eq!(half.format, StorageImageFormat::Rgba16Float);
    let ten_bit = run_viewfinder(
        context,
        &test_config(ViewfinderStorageFormat::PreferTenBitUnorm),
        &request,
    );
    ParityMeasure {
        half: parity_with_pixels(&half, reference.width, reference.height, &reference.pixels),
        ten_bit: parity_with_pixels(&ten_bit, reference.width, reference.height, &reference.pixels),
        ten_bit_format_is_unorm: ten_bit.format == StorageImageFormat::A2B10G10R10Unorm,
    }
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
