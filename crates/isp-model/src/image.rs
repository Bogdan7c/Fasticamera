//! Изображения стадий ISP со строгим смыслом.
//!
//! Каждая стадия эталона принимает и возвращает изображение своего типа, поэтому перепутать
//! стадии нельзя: функция цвета не примет мозаику, кривая передачи не примет сцену в XYZ.
//!
//! - Мозаика (один отсчёт на пиксель, цвет задаёт CFA): [`RawMosaic`] (u16 с сенсора) →
//!   [`CameraMosaic`] (нормирована: чёрный 0, белый 1) → [`WhiteBalancedMosaic`].
//! - Цветные изображения (три отсчёта на пиксель) различаются «пространством» `S`:
//!   [`LinearCameraRgb`] → [`XyzD50Image`] → [`SceneLinearImage`] → [`DisplayLinearImage`] →
//!   [`DisplayEncodedImage`].
//!
//! Пространство — значение, а не только тип: у изображений в основных цветах экрана оно
//! хранит гамму ([`DisplayGamut`]), чтобы метрики и паритет GPU знали, что сравнивают.

use camera_contract::geometry::PixelSize;

use crate::cfa::BayerPattern;

/// Ошибка построения изображения из готовых отсчётов (тесты, readback GPU, кропы).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImageError {
    #[error("пустое изображение {width}×{height}")]
    Empty { width: u32, height: u32 },
    #[error("ожидалось {expected} отсчётов для {width}×{height}, передано {actual}")]
    SampleCountMismatch {
        width: u32,
        height: u32,
        expected: u64,
        actual: u64,
    },
    #[error("мозаика Bayer требует чётных ширины и высоты, передано {width}×{height}")]
    OddMosaicSize { width: u32, height: u32 },
}

/// Проверяет, что размер не пустой и число отсчётов совпадает с ним.
fn check_sample_count(size: PixelSize, actual: usize) -> Result<(), ImageError> {
    if size.width == 0 || size.height == 0 {
        return Err(ImageError::Empty {
            width: size.width,
            height: size.height,
        });
    }
    let expected = size.pixel_count();
    // usize → u64 без потерь на всех поддерживаемых платформах (64 бита).
    let actual = actual as u64;
    if expected != actual {
        return Err(ImageError::SampleCountMismatch {
            width: size.width,
            height: size.height,
            expected,
            actual,
        });
    }
    Ok(())
}

/// Мозаика Bayer повторяется квадратом 2×2, поэтому нечётный размер разрезал бы квадрат.
fn check_even_mosaic(size: PixelSize) -> Result<(), ImageError> {
    if !size.width.is_multiple_of(2) || !size.height.is_multiple_of(2) {
        return Err(ImageError::OddMosaicSize {
            width: size.width,
            height: size.height,
        });
    }
    Ok(())
}

/// Индекс отсчёта (x, y) в плоском массиве в порядке строк.
fn row_major_index(size: PixelSize, x: u32, y: u32) -> usize {
    debug_assert!(x < size.width && y < size.height, "координата вне изображения");
    y as usize * size.width as usize + x as usize
}

/// Сырая мозаика сенсора: u16 как пришли с камеры (RAW10 уже распакован в u16).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMosaic {
    size: PixelSize,
    pattern: BayerPattern,
    samples: Vec<u16>,
}

impl RawMosaic {
    /// Мозаика из отсчётов в порядке строк; размер обязан быть чётным (квадрат 2×2 целиком).
    pub fn new(size: PixelSize, pattern: BayerPattern, samples: Vec<u16>) -> Result<Self, ImageError> {
        check_sample_count(size, samples.len())?;
        check_even_mosaic(size)?;
        Ok(Self { size, pattern, samples })
    }

    pub fn size(&self) -> PixelSize {
        self.size
    }

    pub fn pattern(&self) -> BayerPattern {
        self.pattern
    }

    pub fn samples(&self) -> &[u16] {
        &self.samples
    }

    pub fn sample(&self, x: u32, y: u32) -> u16 {
        self.samples[row_major_index(self.size, x, y)]
    }

    /// Строит нормированную мозаику функцией `(x, y, отсчёт RAW) → f32`. Только для стадий
    /// этого crate: размер уже проверен, поэтому ошибки построения здесь нет.
    pub(crate) fn map_into<T: Copy>(&self, stage: T, mut transform: impl FnMut(u32, u32, u16) -> f32) -> Mosaic<T> {
        let width = self.size.width as usize;
        let samples = self
            .samples
            .iter()
            .enumerate()
            .map(|(index, &sample)| transform((index % width) as u32, (index / width) as u32, sample))
            .collect();
        Mosaic {
            size: self.size,
            pattern: self.pattern,
            samples,
            stage,
        }
    }
}

/// Стадии нормированной мозаики (маркеры типа, без данных).
pub mod mosaic_stage {
    /// После уровней: чёрный 0, белый сенсора 1.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Normalized;

    /// После стадии виньетирования — применённой или честно пропущенной. Отдельный тип
    /// не даёт ни пропустить стадию, ни применить коррекцию дважды.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct ShadingCorrected;

    /// После баланса белого: серый объект даёт равные R, G, B; пересвет обрезан до 1.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct WhiteBalanced;
}

/// Нормированная мозаика стадии `S` (f32, один отсчёт на пиксель).
#[derive(Debug, Clone, PartialEq)]
pub struct Mosaic<S> {
    size: PixelSize,
    pattern: BayerPattern,
    samples: Vec<f32>,
    stage: S,
}

/// Мозаика после уровней: чёрный 0, белый 1, без баланса белого.
pub type CameraMosaic = Mosaic<mosaic_stage::Normalized>;
/// Мозаика после стадии виньетирования — вход баланса белого.
pub type ShadingCorrectedMosaic = Mosaic<mosaic_stage::ShadingCorrected>;
/// Мозаика после баланса белого — вход демозаика.
pub type WhiteBalancedMosaic = Mosaic<mosaic_stage::WhiteBalanced>;

impl<S: Copy> Mosaic<S> {
    /// Мозаика из готовых отсчётов. Нужна тестам, кропам и сверке с readback GPU;
    /// стадии эталона строят мозаики сами.
    pub fn from_samples(
        size: PixelSize,
        pattern: BayerPattern,
        samples: Vec<f32>,
        stage: S,
    ) -> Result<Self, ImageError> {
        check_sample_count(size, samples.len())?;
        check_even_mosaic(size)?;
        Ok(Self {
            size,
            pattern,
            samples,
            stage,
        })
    }

    pub fn size(&self) -> PixelSize {
        self.size
    }

    pub fn pattern(&self) -> BayerPattern {
        self.pattern
    }

    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    pub fn sample(&self, x: u32, y: u32) -> f32 {
        self.samples[row_major_index(self.size, x, y)]
    }

    /// Строит мозаику следующей стадии, вычисляя отсчёты функцией `(x, y, отсчёт) → отсчёт`.
    /// Только для стадий этого crate: снаружи стадию нельзя «объявить» без её формулы.
    pub(crate) fn map_into<T: Copy>(
        &self,
        next_stage: T,
        mut transform: impl FnMut(u32, u32, f32) -> f32,
    ) -> Mosaic<T> {
        let width = self.size.width as usize;
        let samples = self
            .samples
            .iter()
            .enumerate()
            .map(|(index, &sample)| {
                // Индекс плоского массива → координаты (размер ≤ u32 по построению).
                let x = (index % width) as u32;
                let y = (index / width) as u32;
                transform(x, y, sample)
            })
            .collect();
        Mosaic {
            size: self.size,
            pattern: self.pattern,
            samples,
            stage: next_stage,
        }
    }
}

/// Гамма (основные цвета) экрана или файла на выходе. Белая точка у обеих — D65.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayGamut {
    /// IEC 61966-2-1 (основные цвета Rec. 709).
    Srgb,
    /// Display P3 (основные цвета DCI-P3, белый D65) — экран телефона и HEIF у iPhone.
    DisplayP3,
}

/// Пространства цветных изображений. Значения, а не только маркеры: у пространств экрана
/// внутри лежит гамма.
pub mod space {
    use super::DisplayGamut;

    /// Линейный RGB камеры после баланса белого (серый → R = G = B).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct CameraRgb;

    /// CIE XYZ с белой точкой D50 (связующее пространство DNG).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct XyzD50;

    /// Линейный свет сцены в основных цветах экрана: ещё не сжат тоном, может быть > 1
    /// и (вне гаммы) < 0.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct SceneLinear {
        pub gamut: DisplayGamut,
    }

    /// Сцена после мягкого сжатия охвата: каналы ≥ 0. Отдельный тип — сжатие не применить дважды.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct SceneInGamut {
        pub gamut: DisplayGamut,
    }

    /// Линейный свет сцены в рабочем пространстве формирования картинки — Rec. 2020 (D65), решение 176.
    /// Не сжат тоном; вне охвата Rec. 2020 возможны отрицательные компоненты.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct WorkingScene;

    /// Линейный свет экрана после основы AgX, ещё в рабочем пространстве Rec. 2020: 0…1 по смыслу,
    /// но матрица «наружу» может дать небольшой выход за 0…1 — его убирает перевод в цвета выхода.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct WorkingDisplay;

    /// Линейный свет экрана после тона: 0…1, внутри гаммы.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct DisplayLinear {
        pub gamut: DisplayGamut,
    }

    /// Закодированный кривой передачи (OETF) сигнал 0…1 — то, что пишется в буфер экрана/файл.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct DisplayEncoded {
        pub gamut: DisplayGamut,
    }
}

/// Цветное изображение пространства `S` (f32, три отсчёта на пиксель, порядок строк).
#[derive(Debug, Clone, PartialEq)]
pub struct RgbImage<S> {
    size: PixelSize,
    pixels: Vec<[f32; 3]>,
    space: S,
}

/// Демозаик → цвет.
pub type LinearCameraRgb = RgbImage<space::CameraRgb>;
/// Цвет камеры → XYZ D50 → адаптация к экрану.
pub type XyzD50Image = RgbImage<space::XyzD50>;
/// Сцена в рабочем пространстве Rec. 2020 → основа AgX.
pub type WorkingSceneImage = RgbImage<space::WorkingScene>;
/// После основы AgX, в рабочем пространстве → цвета выхода.
pub type WorkingDisplayImage = RgbImage<space::WorkingDisplay>;
/// Сцена в основных цветах экрана → сжатие охвата.
pub type SceneLinearImage = RgbImage<space::SceneLinear>;
/// Сцена внутри охвата → тон.
pub type SceneInGamutImage = RgbImage<space::SceneInGamut>;
/// После тона → кривая передачи.
pub type DisplayLinearImage = RgbImage<space::DisplayLinear>;
/// Итог эталона.
pub type DisplayEncodedImage = RgbImage<space::DisplayEncoded>;

impl<S: Copy> RgbImage<S> {
    /// Изображение из готовых пикселей (тесты, кропы, readback GPU).
    pub fn from_pixels(size: PixelSize, pixels: Vec<[f32; 3]>, space: S) -> Result<Self, ImageError> {
        check_sample_count(size, pixels.len())?;
        Ok(Self { size, pixels, space })
    }

    /// Изображение, чей размер стадия этого crate вывела из уже проверенного входа (демозаик).
    /// Проверка числа пикселей — только в отладке: ошибка здесь — баг стадии, а не данных.
    pub(crate) fn from_stage_output(size: PixelSize, pixels: Vec<[f32; 3]>, space: S) -> Self {
        debug_assert_eq!(size.pixel_count(), pixels.len() as u64, "стадия вывела неверный размер");
        Self { size, pixels, space }
    }

    pub fn size(&self) -> PixelSize {
        self.size
    }

    pub fn space(&self) -> S {
        self.space
    }

    pub fn pixels(&self) -> &[[f32; 3]] {
        &self.pixels
    }

    pub fn pixel(&self, x: u32, y: u32) -> [f32; 3] {
        self.pixels[row_major_index(self.size, x, y)]
    }

    /// Строит изображение следующего пространства попиксельной функцией.
    /// Только для стадий этого crate (см. [`Mosaic::map_into`]).
    pub(crate) fn map_into<T: Copy>(&self, next_space: T, transform: impl FnMut([f32; 3]) -> [f32; 3]) -> RgbImage<T> {
        let pixels = self.pixels.iter().copied().map(transform).collect();
        RgbImage {
            size: self.size,
            pixels,
            space: next_space,
        }
    }
}
