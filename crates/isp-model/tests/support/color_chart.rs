//! Мишень для замера цветных ручек: 24 патча ColorChecker Classic (2005) собраны в синтетический RAW-кадр и идут
//! через ВЕСЬ эталонный конвейер (`IspParams::resolve` → `run_reference_encoded`), поэтому теплота и оттенок
//! (сдвиг белой точки в `resolve`) меряются тем же путём, что насыщенность, полосы и колёса.
//!
//! Мера — «наибольший сдвиг кода канала»: по патчам и каналам берётся наибольшее `|код_с_ручкой − код_без|` из 255
//! в сигнале экрана (Display P3). Патчи заданы значением «баланс белого камеры»: серый предмет под лампой даёт
//! нейтраль AWB, поэтому серые патчи остаются серыми, а цветные несут свой цвет.

use camera_contract::geometry::PixelSize;
use isp_model::cfa::CfaCell;
use isp_model::image::RawMosaic;
use isp_model::look::model::LookModelSettings;
use isp_model::pipeline::run_reference_encoded;
use isp_model::tone::base::ToneBase;
use isp_model::transfer::srgb_decode;
use look_contract::LookSettings;

use super::knob_strength::KnobBench;

/// 24 патча ColorChecker Classic (2005): имя и sRGB 8 бит по публикации BabelColor / X-Rite.
const PATCHES: [(&str, [u8; 3]); 24] = [
    ("dark skin", [115, 82, 68]),
    ("light skin", [194, 150, 130]),
    ("blue sky", [98, 122, 157]),
    ("foliage", [87, 108, 67]),
    ("blue flower", [133, 128, 177]),
    ("bluish green", [103, 189, 170]),
    ("orange", [214, 126, 44]),
    ("purplish blue", [80, 91, 166]),
    ("moderate red", [193, 90, 99]),
    ("purple", [94, 60, 108]),
    ("yellow green", [157, 188, 64]),
    ("orange yellow", [224, 163, 46]),
    ("blue", [56, 61, 150]),
    ("green", [70, 148, 73]),
    ("red", [175, 54, 60]),
    ("yellow", [231, 199, 31]),
    ("magenta", [187, 86, 149]),
    ("cyan", [8, 133, 161]),
    ("white", [243, 243, 242]),
    ("neutral 8", [200, 200, 200]),
    ("neutral 6.5", [160, 160, 160]),
    ("neutral 5", [122, 122, 121]),
    ("neutral 3.5", [85, 85, 85]),
    ("black", [52, 52, 52]),
];
/// Патчей в строке сетки кадра (сетка 6×4).
const COLUMNS: u32 = 6;
/// Сторона квадрата патча, пикселей: с запасом для демозаика, замер — по центру.
const PATCH_SIDE: u32 = 16;
const ROWS: u32 = 4;
/// Масштаб кода экрана.
const CODE_SCALE: f32 = 255.0;

/// Самый сильный сдвиг мишени и где он случился.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChartShift {
    /// Наибольший `|сдвиг|` кода канала, из 255.
    pub codes: f32,
    /// Патч, где он случился.
    pub patch: &'static str,
    /// Средний `|сдвиг|` по всем патчам и каналам, из 255. Наибольший сдвиг упирается в край охвата экрана (самый
    /// цветной патч уже прижат к 0 или 1), а средний продолжает расти, пока ручка хоть что-то меняет.
    pub mean_codes: f32,
}

/// Линейный свет патча «баланс белого камеры» из sRGB 8 бит.
fn linear_patch(rgb: [u8; 3]) -> [f32; 3] {
    rgb.map(|channel| srgb_decode(f32::from(channel) / 255.0))
}

impl KnobBench {
    /// Кадр мишени: каждый патч — квадрат `PATCH_SIDE`², отсчёт канала = чёрный + нейтраль_AWB·свет·(белый − чёрный).
    fn chart_frame(&self) -> RawMosaic {
        let pattern = self.sensor.pattern();
        let levels = &self.frame.levels;
        let neutral = self.neutral.values();
        let size = PixelSize::new(COLUMNS * PATCH_SIDE, ROWS * PATCH_SIDE);
        let mut samples = Vec::new();
        for y in 0..size.height {
            for x in 0..size.width {
                let patch_index = (y / PATCH_SIDE) * COLUMNS + x / PATCH_SIDE;
                let light = linear_patch(PATCHES[patch_index as usize].1);
                let channel = pattern.channel_at(x, y).index();
                let cell = CfaCell::of_pixel(x, y);
                let black = levels.black(cell);
                let level = neutral[channel] as f32 * light[channel];
                samples.push((black + level * (levels.white() - black)).round() as u16);
            }
        }
        RawMosaic::new(size, pattern, samples).expect("размер кадра мишени согласован с числом отсчётов")
    }

    /// Коды центров патчей (из 255, по каналам) при `settings` на основе `base`.
    fn chart_codes(
        &self,
        base: &ToneBase,
        settings: &LookSettings,
        model: &LookModelSettings,
    ) -> Result<Vec<[f32; 3]>, String> {
        let params = self.resolve(base, settings, model)?;
        let image = run_reference_encoded(&self.chart_frame(), &params).map_err(|error| error.to_string())?;
        Ok((0..PATCHES.len() as u32)
            .map(|index| {
                let (column, row) = (index % COLUMNS, index / COLUMNS);
                let centre = (column * PATCH_SIDE + PATCH_SIDE / 2, row * PATCH_SIDE + PATCH_SIDE / 2);
                image.pixel(centre.0, centre.1).map(|channel| channel * CODE_SCALE)
            })
            .collect())
    }

    /// Коды мишени БЕЗ вида на основе `base`: общий базис для серии замеров (нейтраль считается один раз).
    pub fn chart_baseline(&self, base: &ToneBase, model: &LookModelSettings) -> Result<ChartCodes, String> {
        self.chart_codes(base, &LookSettings::NEUTRAL, model).map(ChartCodes)
    }

    /// Наибольший сдвиг кода канала на мишени при `settings` против готового базиса `baseline` (той же основы и модели).
    pub fn measure_chart_against(
        &self,
        baseline: &ChartCodes,
        base: &ToneBase,
        settings: &LookSettings,
        model: &LookModelSettings,
    ) -> Result<ChartShift, String> {
        let touched = self.chart_codes(base, settings, model)?;
        let mut worst = ChartShift {
            codes: 0.0,
            patch: PATCHES[0].0,
            mean_codes: 0.0,
        };
        let mut total = 0.0;
        for (index, (before, after)) in baseline.0.iter().zip(&touched).enumerate() {
            for channel in 0..3 {
                let shift = (after[channel] - before[channel]).abs();
                total += shift;
                if shift > worst.codes {
                    worst = ChartShift {
                        codes: shift,
                        patch: PATCHES[index].0,
                        ..worst
                    };
                }
            }
        }
        worst.mean_codes = total / (baseline.0.len() * 3) as f32;
        Ok(worst)
    }
}

/// Коды мишени без вида (по патчам и каналам, из 255).
#[derive(Debug, Clone, PartialEq)]
pub struct ChartCodes(Vec<[f32; 3]>);
