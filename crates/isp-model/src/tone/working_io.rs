//! Общий шаг рабочего пространства `W` для ВСЕХ моделей основы тона (AgX, ACES, filmic; решения 176, 182).
//!
//! Любая модель основы устроена как «вход в `W` → формулы модели → выход». Вход и выход от модели не зависят,
//! поэтому живут здесь, а не внутри одной из моделей:
//! ```text
//! XYZ D50 ──[xyz_d50_to_working]──▶ W (линейный Rec. 2020)
//!   W ──[apply_input: обрезка нулём | мягкое сжатие охвата W]──▶ вход формул модели
//!   … формулы модели (AgX | ACES | filmic) …
//!   свет экрана в W ──[to_output: W → цвета выхода, мягкое сжатие охвата выхода, clamp 0…1]──▶ OETF
//! ```
//! Раньше ACES и filmic держали внутри себя вложенную `AgxBase` с умолчаниями AgX ради этих шагов (хак);
//! теперь все три модели владеют одним [`WorkingSpaceIo`], а настройки шага — [`WorkingIoSettings`] — собирает
//! каждая модель из своих полей (формат конфига моделей не менялся).
//!
//! Поведение бит в бит прежнее: матрицы считаются теми же функциями [`crate::color::working_space`] в f64 и
//! один раз сужаются до f32; сжатие охвата — [`GamutCompression`].

use crate::color::matrix::{PixelMatrix, SingularMatrix};
use crate::color::working_space::{working_to_output, xyz_d50_to_working};
use crate::gamut::{GamutCompression, GamutCompressionParameters, GamutCompressionSettings, InvalidGamutCompression};
use crate::image::DisplayGamut;

/// Что делать с цветом сцены за охватом рабочего пространства на входе в модель.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkingInputGamut {
    /// Отрицательные компоненты обнуляются (`max(c, 0)`): дёшево, но рвёт оттенок за охватом.
    ClipToZero,
    /// Мягкое сжатие охвата (та же формула, что у [`crate::gamut`]) со своими порогами для Rec. 2020.
    SoftCompress(GamutCompressionSettings),
}

impl WorkingInputGamut {
    /// Пороги мягкого сжатия на входе: сжимаем только цвета у границы Rec. 2020 и за ней. Пороги ACES RGC
    /// (0,80–0,88) подобраны под другое пространство и приглушали бы обычные насыщенные цвета внутри
    /// Rec. 2020 (замер P1: средняя цветность падала на 8 %). `[estimate]`.
    pub const DEFAULT_COMPRESSION: GamutCompressionSettings = GamutCompressionSettings {
        threshold: [0.95; 3],
        distance_limit: [1.25; 3],
        power: 1.2,
    };
}

/// Настройки общего шага: ровно те два поля, которые у каждой модели основы лежат в её настройках.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkingIoSettings {
    /// Вход в `W`: что делать с цветом за охватом Rec. 2020.
    pub input_gamut: WorkingInputGamut,
    /// Мягкое сжатие охвата выхода (после матрицы в цвета экрана, перед обрезкой 0…1).
    pub output_gamut_compression: GamutCompressionSettings,
}

/// Общий шаг не строится.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum InvalidWorkingIo {
    #[error("сжатие охвата на входе: {0}")]
    InputGamut(InvalidGamutCompression),
    #[error("сжатие охвата выхода: {0}")]
    OutputGamut(InvalidGamutCompression),
    #[error("матрицы рабочего пространства вырождены")]
    SingularMatrix(#[from] SingularMatrix),
}

/// Разрешённый вход.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ResolvedInput {
    ClipToZero,
    SoftCompress(GamutCompression),
}

/// Числа общего шага для шейдера: ровно те f32, что подставляют [`WorkingSpaceIo::apply_input`] и
/// [`WorkingSpaceIo::to_output`]. Только чтение: собрать [`WorkingSpaceIo`] из них нельзя.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct WorkingIoParameters {
    /// `XYZ D50 → W`, по строкам.
    pub xyz_d50_to_working: [[f32; 3]; 3],
    /// `W → цвета выхода`, по строкам.
    pub working_to_output: [[f32; 3]; 3],
    /// Сжатие охвата на входе: `None` — обрезка нулём.
    pub input_compression: Option<GamutCompressionParameters>,
    /// Мягкое сжатие охвата выхода (после него — обрезка 0…1).
    pub output_compression: GamutCompressionParameters,
}

/// Разрешённый общий шаг рабочего пространства: матрицы `XYZ D50 → W`, `W → выход` и два сжатия охвата.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkingSpaceIo {
    xyz_d50_to_working: PixelMatrix,
    working_to_output: PixelMatrix,
    input: ResolvedInput,
    output_compression: GamutCompression,
    output_gamut: DisplayGamut,
}

impl WorkingSpaceIo {
    /// Разрешает настройки для гаммы выхода. Матрицы считаются в f64 и сужаются до f32 один раз.
    pub fn resolve(settings: &WorkingIoSettings, output_gamut: DisplayGamut) -> Result<Self, InvalidWorkingIo> {
        let input = match settings.input_gamut {
            WorkingInputGamut::ClipToZero => ResolvedInput::ClipToZero,
            WorkingInputGamut::SoftCompress(compression) => ResolvedInput::SoftCompress(
                GamutCompression::resolve(&compression).map_err(InvalidWorkingIo::InputGamut)?,
            ),
        };
        Ok(Self {
            xyz_d50_to_working: xyz_d50_to_working()?.to_f32(),
            working_to_output: working_to_output(output_gamut)?.to_f32(),
            input,
            output_compression: GamutCompression::resolve(&settings.output_gamut_compression)
                .map_err(InvalidWorkingIo::OutputGamut)?,
            output_gamut,
        })
    }

    /// Гамма выхода, под которую посчитана матрица `W → выход`.
    pub fn output_gamut(&self) -> DisplayGamut {
        self.output_gamut
    }

    /// Числа общего шага для GPU (см. [`WorkingIoParameters`]).
    pub fn parameters(&self) -> WorkingIoParameters {
        WorkingIoParameters {
            xyz_d50_to_working: self.xyz_d50_to_working.rows,
            working_to_output: self.working_to_output.rows,
            input_compression: match self.input {
                ResolvedInput::ClipToZero => None,
                ResolvedInput::SoftCompress(compression) => Some(compression.parameters()),
            },
            output_compression: self.output_compression.parameters(),
        }
    }

    /// `XYZ D50 → W` одного пикселя.
    pub fn xyz_d50_to_working(&self, xyz_d50: [f32; 3]) -> [f32; 3] {
        self.xyz_d50_to_working.apply(xyz_d50)
    }

    /// Вход в `W`: обрезка нулём или мягкое сжатие охвата.
    pub fn apply_input(&self, scene: [f32; 3]) -> [f32; 3] {
        match self.input {
            ResolvedInput::ClipToZero => scene.map(|channel| channel.max(0.0)),
            ResolvedInput::SoftCompress(compression) => compression.compress_pixel(scene),
        }
    }

    /// Только матрица `W → цвета выхода`, без сжатия и обрезки (нужна модели, которой надо знать, что получится
    /// на выходе до обрезки: filmic вписывает цвет в охват выхода).
    pub fn working_to_output_colors(&self, display_working: [f32; 3]) -> [f32; 3] {
        self.working_to_output.apply(display_working)
    }

    /// Последний шаг: `W → цвета выхода`, мягкое сжатие охвата выхода и обрезка 0…1.
    pub fn to_output(&self, display_working: [f32; 3]) -> [f32; 3] {
        self.output_compression
            .compress_pixel(self.working_to_output_colors(display_working))
            .map(|channel| channel.clamp(0.0, 1.0))
    }
}

#[cfg(test)]
#[path = "working_io_tests.rs"]
mod tests;
