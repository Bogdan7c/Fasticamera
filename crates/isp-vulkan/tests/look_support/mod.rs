//! Общие помощники тестов слоя вида на GPU: значения ручек, таблицы, сцены с разверткой по оттенку.
//!
//! Подключается как `mod look_support;` из тестов видоискателя, качественного пути и снимка. Всё здесь строит
//! ВХОДЫ (ручки, таблицы, кадры); эталон и сравнение — в самих тестах.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "общий модуль тестов: каждый тест берёт своё подмножество, а падение и есть способ сообщить о провале"
)]

use std::sync::Arc;

use camera_contract::geometry::PixelSize;
use isp_model::color::dng::CameraNeutral;
use isp_model::image::RawMosaic;
use isp_model::sensor::SensorDescription;

use isp_model::look::LookControls;
use isp_model::look::lut::LookLut;
use isp_model::look::model::LookModelSettings;
use look_contract::{
    BandAdjustment, BandAdjustments, ColorWheel, ColorWheels, LookKnob, LookSettings, WheelPush, WheelZone,
};

/// Ручка со значением `value` ∈ [−1, 1].
pub fn knob(value: f32) -> LookKnob {
    LookKnob::new(value).unwrap()
}

/// Модель вида по умолчанию (8 полос).
pub fn model() -> Arc<LookModelSettings> {
    Arc::new(LookModelSettings::default())
}

/// Вид с этими ручками, без таблицы.
pub fn controls(settings: LookSettings) -> LookControls {
    LookControls::new(settings, model())
}

/// Вид только с насыщенностью и сочностью.
pub fn saturation_vibrance(saturation: f32, vibrance: f32) -> LookControls {
    controls(LookSettings {
        saturation: knob(saturation),
        vibrance: knob(vibrance),
        ..LookSettings::NEUTRAL
    })
}

/// Правка одной полосы модели по номеру (в порядке модели).
pub fn band(index: usize, hue: f32, saturation: f32, luminance: f32) -> BandAdjustment {
    BandAdjustment {
        band: model().bands[index].id.clone(),
        hue: knob(hue),
        saturation: knob(saturation),
        luminance: knob(luminance),
    }
}

/// Вид с правками полос.
pub fn bands(adjustments: Vec<BandAdjustment>) -> LookControls {
    controls(LookSettings {
        bands: BandAdjustments::new(adjustments).unwrap(),
        ..LookSettings::NEUTRAL
    })
}

/// Вид, в котором ВСЕ полосы получили одни и те же правки.
pub fn all_bands(hue: f32, saturation: f32, luminance: f32) -> LookControls {
    bands(
        (0..model().bands.len())
            .map(|index| band(index, hue, saturation, luminance))
            .collect(),
    )
}

/// Число узлов на ось таблиц тестов: небольшое, но больше 2, с «внутренними» ячейками.
pub const TEST_LUT_SIZE: u32 = 9;

/// Таблица-«единица»: узел равен своей координате.
pub fn identity_lut() -> Arc<LookLut> {
    LookLut::identity(TEST_LUT_SIZE, 65).unwrap()
}

/// Известная таблица с заметным, но гладким искажением: перестановка и сдвиг каналов по формуле от координат
/// узла. Не аффинна (есть квадраты), поэтому тетраэдры по-разному дают разные ответы — проверка действительно
/// ловит ошибку порядка осей и раскладки узлов.
pub fn known_lut() -> Arc<LookLut> {
    let size = TEST_LUT_SIZE;
    let last = (size - 1) as f32;
    let mut nodes = Vec::new();
    for blue in 0..size {
        for green in 0..size {
            for red in 0..size {
                let (r, g, b) = (red as f32 / last, green as f32 / last, blue as f32 / last);
                nodes.push([
                    (0.85 * g + 0.15 * r * r).clamp(0.0, 1.0),
                    (0.9 * b + 0.1 * g * b).clamp(0.0, 1.0),
                    (0.8 * r + 0.2 * b * b).clamp(0.0, 1.0),
                ]);
            }
        }
    }
    LookLut::new(size, nodes, 65).unwrap()
}

/// Те же управления с таблицей.
pub fn with_lut(controls: LookControls, lut: Arc<LookLut>) -> LookControls {
    controls.with_lut(lut)
}

// Свет сцен «развёртка по оттенку» и «кожа и пастели» живёт в библиотеке линейки паритета (их же гоняет приложение).
#[allow(unused_imports, reason = "каждый тестовый бинарь использует свою часть помощников")]
pub use isp_vulkan::saturation::parity::{hsv_to_rgb, hue_sweep_light, skin_and_pastels_light};

/// Мозаика размера `width`×`height` с разверткой по оттенку ([`hue_sweep_light`]) для сенсора `sensor`.
pub fn hue_sweep_mosaic(
    sensor: &isp_model::sensor::SensorDescription,
    neutral: isp_model::color::dng::CameraNeutral,
    width: u32,
    height: u32,
) -> isp_model::image::RawMosaic {
    let levels = sensor.static_levels();
    let pattern = sensor.pattern();
    let values = neutral.values();
    let samples: Vec<u16> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .map(|(x, y)| {
            let channel = pattern.channel_at(x, y).index();
            let light = hue_sweep_light(x, y, width, height, channel, values);
            let black = f64::from(levels.black_pattern()[(y as usize % 2) * 2 + x as usize % 2]);
            (black + light * (f64::from(levels.white()) - black)).round() as u16
        })
        .collect();
    isp_model::image::RawMosaic::new(
        camera_contract::geometry::PixelSize::new(width, height),
        pattern,
        samples,
    )
    .unwrap()
}

/// Серая шкала на весь диапазон: яркость растёт слева направо от почти чёрного (носок кривой) до пересвета 1,3
/// (плечо); нейтраль кадра — серый остаётся серым. Развёртка по оттенку тёмных тонов не достаёт, а ручка «тени» живёт
/// именно в них.
pub fn gray_ramp_mosaic(sensor: &SensorDescription, neutral: CameraNeutral, width: u32, height: u32) -> RawMosaic {
    let levels = sensor.static_levels();
    let pattern = sensor.pattern();
    let values = neutral.values();
    let brightest = values.iter().copied().fold(f64::MIN, f64::max);
    let samples: Vec<u16> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .map(|(x, y)| {
            let channel = pattern.channel_at(x, y).index();
            let light = (f64::from(x) / f64::from(width - 1) * 1.3 * values[channel] / brightest).min(1.0);
            let black = f64::from(levels.black_pattern()[(y as usize % 2) * 2 + x as usize % 2]);
            (black + light * (f64::from(levels.white()) - black)).round() as u16
        })
        .collect();
    RawMosaic::new(PixelSize::new(width, height), pattern, samples).unwrap()
}

/// Колесо: толчок цвета в сторону `hue_degrees` силой `strength` (0…1) и кольцо яркости `ring` (−1…1).
pub fn wheel(hue_degrees: f32, strength: f32, ring: f32) -> ColorWheel {
    ColorWheel {
        push: WheelPush::from_hue_degrees(hue_degrees, strength).unwrap(),
        ring: knob(ring),
    }
}

/// Вид только с колёсами `wheels`.
pub fn wheels_look(wheels: ColorWheels) -> LookControls {
    controls(LookSettings {
        wheels,
        ..LookSettings::NEUTRAL
    })
}

/// Вид, в котором тронуто одно колесо зоны `zone`.
pub fn single_wheel_look(zone: WheelZone, wheel: ColorWheel) -> LookControls {
    wheels_look(ColorWheels::NEUTRAL.with_wheel(zone, wheel))
}

/// Все четыре колеса сразу, каждое со своим цветом и кольцом.
pub fn all_wheels() -> ColorWheels {
    ColorWheels::NEUTRAL
        .with_wheel(WheelZone::Lift, wheel(30.0, 0.9, 0.6))
        .with_wheel(WheelZone::Gamma, wheel(150.0, 0.7, -0.4))
        .with_wheel(WheelZone::Gain, wheel(270.0, 0.8, 0.5))
        .with_wheel(WheelZone::Offset, wheel(330.0, 0.5, -0.3))
}

/// Виды паритета колёс (общие для видоискателя, качественного пути и снимка): каждое колесо отдельно с сильным
/// толчком и кольцом, все четыре вместе с насыщенностью и полосой, кольца на краях ±1 (без толчка и с ним).
pub fn wheels_parity_cases() -> Vec<(String, LookControls)> {
    let mut cases = Vec::new();
    for (zone, name, hue) in [
        (WheelZone::Lift, "Lift", 30.0),
        (WheelZone::Gamma, "Gamma", 150.0),
        (WheelZone::Gain, "Gain", 270.0),
        (WheelZone::Offset, "Offset", 330.0),
    ] {
        cases.push((
            format!("колесо {name}: толчок+кольцо"),
            single_wheel_look(zone, wheel(hue, 1.0, 1.0)),
        ));
        cases.push((
            format!("колесо {name}: кольцо +1"),
            single_wheel_look(zone, wheel(0.0, 0.0, 1.0)),
        ));
        cases.push((
            format!("колесо {name}: кольцо −1 и толчок"),
            single_wheel_look(zone, wheel(hue + 180.0, 1.0, -1.0)),
        ));
    }
    cases.push(("четыре колеса".to_owned(), wheels_look(all_wheels())));
    cases.push((
        "четыре колеса + насыщенность + полоса".to_owned(),
        controls(LookSettings {
            wheels: all_wheels(),
            saturation: knob(0.5),
            vibrance: knob(0.3),
            bands: BandAdjustments::new(vec![band(1, 0.6, -0.4, 0.3)]).unwrap(),
            ..LookSettings::NEUTRAL
        }),
    ));
    cases
}

/// Сколько углов у четырёх ручек тона (каждая в −1 или +1).
pub const TONE_CORNER_COUNT: u32 = 16;

/// Ручки тона (контраст, средние, тени, света) со значениями `contrast`, `midtones`, `shadows`, `highlights`.
pub fn tone_settings(contrast: f32, midtones: f32, shadows: f32, highlights: f32) -> LookSettings {
    LookSettings {
        contrast: knob(contrast),
        midtones: knob(midtones),
        shadows: knob(shadows),
        highlights: knob(highlights),
        ..LookSettings::NEUTRAL
    }
}

/// Угол ручек тона номер `corner` (`0..16`): бит 0 — контраст, 1 — средние, 2 — тени, 3 — света; бит 1 — край +1.
pub fn tone_corner_settings(corner: u32) -> LookSettings {
    let edge = |bit: u32| if corner >> bit & 1 == 1 { 1.0 } else { -1.0 };
    tone_settings(edge(0), edge(1), edge(2), edge(3))
}

/// То же угол `corner`, но вид (с моделью по умолчанию), а не значения ручек.
pub fn tone_corner(corner: u32) -> LookControls {
    controls(tone_corner_settings(corner))
}

/// Только тон: четыре ручки вида.
pub fn tone_look(contrast: f32, midtones: f32, shadows: f32, highlights: f32) -> LookControls {
    controls(tone_settings(contrast, midtones, shadows, highlights))
}

/// Угол «контраст −1, средние −1, света −1» (тени — любые): кривая у белого превращается в обрыв.
///
/// Света −1 растягивают расстояние до белого в `2^3,5 ≈ 11` раз, контраст −1 уменьшает наклон на сером в `2^2,32 ≈ 5` раз, а
/// средние −1 дают ещё ×8 у белого: наклон кривой на `W ∈ [0,999; 1)` — 240…450 (кривая за одну тысячную до белого уходит
/// с 0,67 в 1,0). Разница GPU и эталона в 1–3 ULP входа (основа AgX, filmic считаются на GPU и CPU по-разному в последних
/// битах) вырастает там в 1e-5…1e-4 выхода, то есть больше действующего допуска паритета 1e-5. Это условность угла, а не
/// ошибка стадии (все остальные углы держат допуск); у этих углов свой названный допуск
/// `ErrorAllowance::SteepToneAtWhite` — 2e-4, меньше половины ступени 10-битного экрана.
pub fn tone_corner_is_ill_conditioned(corner: u32) -> bool {
    const CONTRAST: u32 = 1 << 0;
    const MIDTONES: u32 = 1 << 1;
    const HIGHLIGHTS: u32 = 1 << 3;
    corner & (CONTRAST | MIDTONES | HIGHLIGHTS) == 0
}
