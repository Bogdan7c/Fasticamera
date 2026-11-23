//! Синтетические сцены видоискателя и общий паритет «GPU против эталона» (обе основы тона).

use super::*;

/// Допуск ошибки вычислений GPU сверх шага хранения (см. модуль).
pub const MATH_EXCESS_TOLERANCE: f32 = 1.0e-5;
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

/// Прогоняет кадр в обоих форматах и проверяет допуски; возвращает (fp16, 10 бит) max |Δ|.
pub fn assert_viewfinder_parity(
    context: &GpuContext,
    name: &str,
    raw: &RawFrame,
    params: &IspParams,
    spec: &ParamsSpec,
) -> (f32, f32) {
    let request = RunRequest::whole_frame(raw, params, spec.display());
    let reference = reference_upright_encoded(raw, raw.full_rect(), SensorOrientation::Degrees0, params);

    let half = run_viewfinder(context, &test_config(ViewfinderStorageFormat::HalfFloatOnly), &request);
    assert_eq!(half.format, StorageImageFormat::Rgba16Float);
    let half_parity = parity_with_pixels(&half, reference.width, reference.height, &reference.pixels);
    eprintln!(
        "{name} fp16: max |Δ| = {:.3e} ({:.3} LSB), доля > 1 LSB = {:.4}, сверх шага fp16 = {:.2e}",
        half_parity.max_abs,
        half_parity.max_abs / ONE_LSB,
        half_parity.over_one_lsb,
        half_parity.math_excess
    );
    assert!(
        half_parity.math_excess <= MATH_EXCESS_TOLERANCE,
        "{name} fp16: {half_parity:?}"
    );
    assert!(
        half_parity.max_abs <= HALF_LSB + MATH_EXCESS_TOLERANCE,
        "{name} fp16: {half_parity:?}"
    );
    assert_eq!(half_parity.over_one_lsb, 0.0, "{name} fp16: {half_parity:?}");

    let ten_bit = run_viewfinder(
        context,
        &test_config(ViewfinderStorageFormat::PreferTenBitUnorm),
        &request,
    );
    let ten_bit_parity = parity_with_pixels(&ten_bit, reference.width, reference.height, &reference.pixels);
    eprintln!(
        "{name} {:?}: max |Δ| = {:.3e} ({:.3} LSB), доля > 1 LSB = {:.4}, сверх полшага = {:.2e}",
        ten_bit.format,
        ten_bit_parity.max_abs,
        ten_bit_parity.max_abs / ONE_LSB,
        ten_bit_parity.over_one_lsb,
        ten_bit_parity.math_excess
    );
    // Квантование 10 бит — полшага (0,5 LSB) + математика; больше — ошибка формулы.
    assert_eq!(
        ten_bit.format,
        StorageImageFormat::A2B10G10R10Unorm,
        "RADV пишет rgb10_a2"
    );
    assert!(
        ten_bit_parity.math_excess <= MATH_EXCESS_TOLERANCE,
        "{name} 10 бит: {ten_bit_parity:?}"
    );
    assert!(
        ten_bit_parity.max_abs <= HALF_LSB + MATH_EXCESS_TOLERANCE,
        "{name} 10 бит: {ten_bit_parity:?}"
    );
    (half_parity.max_abs, ten_bit_parity.max_abs)
}
