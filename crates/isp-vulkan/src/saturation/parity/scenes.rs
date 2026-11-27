//! Синтетические сцены линейки паритета: те «худшие» входы, на которых тонкие допуски (полосы оттенка, колёса цвета,
//! ручки тона) держатся или нет.
//!
//! Каждая сцена строится от калибровки кадра ([`SceneCalibration`]): раскладка CFA, уровни чёрного и белого, нейтраль
//! баланса белого. Нейтраль нужна, чтобы серый объект остался серым после баланса белого (иначе «серая шкала» была бы
//! цветной). Имена сцен — стабильные ASCII-идентификаторы (их читают отчёты стенда на телефоне).

use camera_contract::geometry::PixelSize;
use isp_model::cfa::BayerPattern;
use isp_model::test_patterns::color_bars;

use super::frame::SyntheticRawFrame;

/// Ширина кадров синтетики.
pub const WIDTH_OF_SYNTHETIC_SCENES: u32 = 64;
/// Высота кадров синтетики.
pub const HEIGHT_OF_SYNTHETIC_SCENES: u32 = 48;

/// От чего зависят сцены: сенсор и баланс белого кадра.
#[derive(Debug, Clone, Copy)]
pub struct SceneCalibration {
    /// Раскладка цветных фильтров.
    pub pattern: BayerPattern,
    /// Чёрный уровень по четырём ячейкам паттерна.
    pub black: [f32; 4],
    /// Белый уровень.
    pub white: f32,
    /// Нейтраль камеры (сигнал серого объекта по R, G, B).
    pub neutral: [f64; 3],
}

impl SceneCalibration {
    /// Уровни в форме, которую принимает [`SyntheticRawFrame::from_linear`].
    pub fn levels(&self) -> ([f32; 4], f32) {
        (self.black, self.white)
    }

    /// Самый яркий канал нейтрали: серый объект нормируется на него, чтобы не вылезти за белый.
    fn brightest_neutral(&self) -> f64 {
        self.neutral.iter().copied().fold(f64::MIN, f64::max)
    }

    /// Кадр размера сцен синтетики из функции линейного света `(x, y, channel)`.
    fn frame(&self, linear: impl Fn(u32, u32, usize) -> f64) -> SyntheticRawFrame {
        SyntheticRawFrame::from_linear(
            WIDTH_OF_SYNTHETIC_SCENES,
            HEIGHT_OF_SYNTHETIC_SCENES,
            self.pattern,
            self.levels(),
            linear,
        )
    }
}

/// Серая шкала: серый объект под нейтралью, яркость 0…1,1 слева направо (правый край — пересвет).
fn gray_ramp(calibration: &SceneCalibration) -> SyntheticRawFrame {
    let (width, brightest, neutral) = (
        WIDTH_OF_SYNTHETIC_SCENES,
        calibration.brightest_neutral(),
        calibration.neutral,
    );
    calibration.frame(|x, _, channel| f64::from(x) / f64::from(width - 1) * 1.1 * neutral[channel] / brightest)
}

/// Цветные полосы `isp-model` (7 цветов) на уровне 0,6, строки — разная яркость 0,2…1.
fn color_bar_scene(calibration: &SceneCalibration) -> SyntheticRawFrame {
    let size = PixelSize::new(WIDTH_OF_SYNTHETIC_SCENES, HEIGHT_OF_SYNTHETIC_SCENES);
    let bars =
        color_bars(size, 0.6).unwrap_or_else(|_| unreachable!("полосы 64x48 строятся: размер и уровень допустимы"));
    calibration.frame(|x, y, channel| {
        let row_level = 0.2 + 0.8 * f64::from(y) / f64::from(HEIGHT_OF_SYNTHETIC_SCENES - 1);
        f64::from(bars.pixel(x, y)[channel]) * row_level / 0.6
    })
}

/// Пересветы: полностью насыщенные ячейки, насыщен один канал, сильно за белым у красного.
fn highlights(calibration: &SceneCalibration) -> SyntheticRawFrame {
    let (brightest, neutral) = (calibration.brightest_neutral(), calibration.neutral);
    calibration.frame(|x, y, channel| {
        let band = x * 4 / WIDTH_OF_SYNTHETIC_SCENES;
        match (band, channel) {
            (0, _) => 1.0,
            (1, 0) => 1.0,
            (1, _) => 0.4 + 0.01 * f64::from(y),
            (2, 2) => 0.98,
            (2, _) => 0.3,
            _ => 0.9 * neutral[channel] / brightest,
        }
    })
}

/// Около чёрного: значения от black−3 до black+20 (ниже чёрного обрезаются уровнями).
fn near_black(calibration: &SceneCalibration) -> SyntheticRawFrame {
    SyntheticRawFrame::from_fn(
        WIDTH_OF_SYNTHETIC_SCENES,
        HEIGHT_OF_SYNTHETIC_SCENES,
        calibration.pattern,
        |x, y, channel| {
            let base = 61 + ((x + 3 * y + channel as u32) % 24);
            base as u16
        },
    )
}

/// Четыре базовые сцены синтетики: имя (ASCII) и кадр. Порядок важен: серая шкала — первая.
pub fn synthetic_scenes(calibration: &SceneCalibration) -> Vec<(&'static str, SyntheticRawFrame)> {
    vec![
        ("gray_ramp", gray_ramp(calibration)),
        ("color_bars", color_bar_scene(calibration)),
        ("highlights", highlights(calibration)),
        ("near_black", near_black(calibration)),
    ]
}

/// RGB (0…1) цвета с оттенком `hue` (доля круга 0…1), насыщенностью `saturation` и значением `value` (HSV).
pub fn hsv_to_rgb(hue: f64, saturation: f64, value: f64) -> [f64; 3] {
    let sector = (hue.rem_euclid(1.0) * 6.0).floor();
    let fraction = hue.rem_euclid(1.0) * 6.0 - sector;
    let (p, q, t) = (
        value * (1.0 - saturation),
        value * (1.0 - saturation * fraction),
        value * (1.0 - saturation * (1.0 - fraction)),
    );
    match sector as u32 {
        0 => [value, t, p],
        1 => [q, value, p],
        2 => [p, value, t],
        3 => [p, q, value],
        4 => [t, p, value],
        _ => [value, p, q],
    }
}

/// Линейный свет сенсора в пикселе `(x, y)` канала `channel` для развёртки по оттенку: по горизонтали полный круг
/// HSV (со стыком 360° → 0°), по вертикали — яркость от тёмной к светлой; свет умножен на нейтраль кадра, поэтому
/// серый останется серым после баланса белого.
pub fn hue_sweep_light(x: u32, y: u32, width: u32, height: u32, channel: usize, neutral: [f64; 3]) -> f64 {
    let brightest = neutral.iter().copied().fold(f64::MIN, f64::max);
    let hue = f64::from(x) / f64::from(width);
    let value = 0.12 + 0.55 * f64::from(y) / f64::from(height - 1);
    hsv_to_rgb(hue, 0.85, value)[channel] * neutral[channel] / brightest
}

/// Линейный свет сенсора для сцены «кожа и пастели»: четыре столбца — два оттенка кожи (умеренная насыщенность) и
/// два малоцветных пастельных (зелёный, голубой); по вертикали — яркость.
pub fn skin_and_pastels_light(x: u32, y: u32, width: u32, height: u32, channel: usize, neutral: [f64; 3]) -> f64 {
    let brightest = neutral.iter().copied().fold(f64::MIN, f64::max);
    let level = 0.25 + 0.5 * f64::from(y) / f64::from(height - 1);
    let rgb = match x * 4 / width {
        0 => hsv_to_rgb(0.08, 0.45, level),
        1 => hsv_to_rgb(0.1, 0.3, level),
        2 => hsv_to_rgb(0.4, 0.12, level),
        _ => hsv_to_rgb(0.58, 0.08, level),
    };
    rgb[channel] * neutral[channel] / brightest
}

/// Развёртка по оттенку (худший случай полос оттенка 14a).
pub fn hue_sweep_scene(calibration: &SceneCalibration) -> SyntheticRawFrame {
    let neutral = calibration.neutral;
    calibration.frame(|x, y, channel| {
        hue_sweep_light(
            x,
            y,
            WIDTH_OF_SYNTHETIC_SCENES,
            HEIGHT_OF_SYNTHETIC_SCENES,
            channel,
            neutral,
        )
    })
}

/// Кожа и малоцветные цвета рядом.
pub fn skin_and_pastels_scene(calibration: &SceneCalibration) -> SyntheticRawFrame {
    let neutral = calibration.neutral;
    calibration.frame(|x, y, channel| {
        skin_and_pastels_light(
            x,
            y,
            WIDTH_OF_SYNTHETIC_SCENES,
            HEIGHT_OF_SYNTHETIC_SCENES,
            channel,
            neutral,
        )
    })
}

/// Яркие насыщенные цвета: полосы с яркостью от почти чёрного до сильного пересвета (худший случай ручек тона 14l).
pub fn bright_saturated_scene(calibration: &SceneCalibration) -> SyntheticRawFrame {
    let size = PixelSize::new(WIDTH_OF_SYNTHETIC_SCENES, HEIGHT_OF_SYNTHETIC_SCENES);
    let bars =
        color_bars(size, 1.0).unwrap_or_else(|_| unreachable!("полосы 64x48 строятся: размер и уровень допустимы"));
    calibration.frame(|x, y, channel| {
        f64::from(bars.pixel(x, y)[channel]) * (0.02 + 1.5 * f64::from(y) / f64::from(HEIGHT_OF_SYNTHETIC_SCENES - 1))
    })
}

/// Сцены паритета вида: четыре базовые, развёртка по оттенку, кожа и пастели.
pub fn look_scenes(calibration: &SceneCalibration) -> Vec<(&'static str, SyntheticRawFrame)> {
    let mut scenes = synthetic_scenes(calibration);
    scenes.push(("hue_sweep", hue_sweep_scene(calibration)));
    scenes.push(("skin_and_pastels", skin_and_pastels_scene(calibration)));
    scenes
}

/// Три сцены паритета тона: серая шкала, яркие насыщенные, развёртка по оттенку.
pub fn tone_scenes(calibration: &SceneCalibration) -> Vec<(&'static str, SyntheticRawFrame)> {
    vec![
        ("gray_ramp", gray_ramp(calibration)),
        ("bright_saturated", bright_saturated_scene(calibration)),
        ("hue_sweep", hue_sweep_scene(calibration)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calibration() -> SceneCalibration {
        SceneCalibration {
            pattern: BayerPattern::BGGR,
            black: [64.0; 4],
            white: 1023.0,
            neutral: [0.515625, 1.0, 0.4990234375],
        }
    }

    #[test]
    fn base_scenes_are_ordered_have_the_documented_size_and_differ_from_each_other() {
        let scenes = synthetic_scenes(&calibration());
        let names: Vec<_> = scenes.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, ["gray_ramp", "color_bars", "highlights", "near_black"]);
        for (name, frame) in &scenes {
            assert_eq!(
                (frame.width, frame.height),
                (WIDTH_OF_SYNTHETIC_SCENES, HEIGHT_OF_SYNTHETIC_SCENES),
                "{name}"
            );
            assert_eq!(frame.samples.len(), (frame.width * frame.height) as usize, "{name}");
        }
        for (index, (name, frame)) in scenes.iter().enumerate() {
            for (other_name, other) in scenes.iter().skip(index + 1) {
                assert_ne!(frame.samples, other.samples, "{name} и {other_name} совпали");
            }
        }
    }

    #[test]
    fn gray_ramp_grows_left_to_right_and_clips_at_white() {
        let (_, ramp) = synthetic_scenes(&calibration()).remove(0);
        let row_sum = |x: u32| -> u32 {
            (0..HEIGHT_OF_SYNTHETIC_SCENES)
                .map(|y| u32::from(ramp.samples[(y * ramp.width + x) as usize]))
                .sum()
        };
        assert!(row_sum(0) < row_sum(WIDTH_OF_SYNTHETIC_SCENES / 2));
        assert!(row_sum(WIDTH_OF_SYNTHETIC_SCENES / 2) < row_sum(WIDTH_OF_SYNTHETIC_SCENES - 1));
        assert_eq!(
            ramp.samples.iter().copied().max(),
            Some(1023),
            "правый край — пересвет, обрезан белым"
        );
    }

    #[test]
    fn hue_sweep_wraps_the_hue_circle_and_covers_every_channel() {
        let neutral = [0.5, 1.0, 0.5];
        let light = |x: u32, channel: usize| hue_sweep_light(x, 24, 64, 48, channel, neutral);
        assert!(
            light(0, 0) > light(0, 1) && light(0, 0) > light(0, 2),
            "оттенок 0 — красный"
        );
        assert!(light(21, 1) > light(21, 0), "около 1/3 круга — зелёный");
        assert!(light(42, 2) > light(42, 0), "около 2/3 круга — синий");
    }

    #[test]
    fn look_scenes_extend_the_base_scenes_and_tone_scenes_start_with_the_gray_ramp() {
        let calibration = calibration();
        let look: Vec<_> = look_scenes(&calibration).into_iter().map(|(name, _)| name).collect();
        assert_eq!(
            look,
            [
                "gray_ramp",
                "color_bars",
                "highlights",
                "near_black",
                "hue_sweep",
                "skin_and_pastels"
            ]
        );
        let tone: Vec<_> = tone_scenes(&calibration).into_iter().map(|(name, _)| name).collect();
        assert_eq!(tone, ["gray_ramp", "bright_saturated", "hue_sweep"]);
    }
}
