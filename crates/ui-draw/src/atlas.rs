//! SDF-атлас глифов и иконок, построенный при сборке (решение 88).
//!
//! Содержимое строит `build.rs` по манифесту `assets/bake.json` и кладёт байты в сборку. Здесь —
//! общие типы, которыми пользуются вёрстка текста, список рисования и GPU-рендер, и доступ к самому атласу.
//!
//! Кодирование байта: `128` — край формы, внутри формы меньше, снаружи больше; шаг линейный,
//! `±spread_texels` текселей соответствуют `128 ± 127`; `255` — далеко снаружи. Расстояние со знаком
//! в текселях: `(байт − 128) / 127 · spread_texels` (внутри — отрицательное, как у `shape` в `ui-draw`).

/// Сгенерированные константы атласа (`OUT_DIR/atlas_meta.rs`).
mod baked {
    include!(concat!(env!("OUT_DIR"), "/atlas_meta.rs"));
}

/// Область атласа одного глифа или иконки: нормированные координаты текстуры (0..1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AtlasRegion {
    /// Левый верхний угол в долях ширины/высоты атласа.
    pub uv_min: [f32; 2],
    /// Правый нижний угол в долях ширины/высоты атласа.
    pub uv_max: [f32; 2],
}

/// Запечённый SDF-атлас: одноканальная текстура и параметры её кодирования.
#[derive(Debug, Clone, Copy)]
pub struct SdfAtlas {
    /// Ширина, текселей.
    pub width: u32,
    /// Высота, текселей.
    pub height: u32,
    /// Байты построчно, `width × height`, один канал.
    pub texels: &'static [u8],
    /// До скольких текселей от края формы хранится расстояние (поле вокруг контура).
    pub spread_texels: f32,
    /// Сколько текселей атласа приходится на один em шрифта.
    pub em_texels: f32,
}

static ATLAS: SdfAtlas = SdfAtlas {
    width: baked::WIDTH_TEXELS,
    height: baked::HEIGHT_TEXELS,
    texels: baked::TEXELS,
    spread_texels: baked::SPREAD_TEXELS as f32,
    em_texels: baked::EM_TEXELS as f32,
};

/// Текселей на em как `f32` — для вёрстки.
pub(crate) const EM_TEXELS_F32: f32 = baked::EM_TEXELS as f32;
/// Поле расстояний, текселей — для иконок.
pub(crate) const SPREAD_TEXELS_F32: f32 = baked::SPREAD_TEXELS as f32;
/// Сторона квадрата viewBox иконки в атласе, текселей, без поля расстояний.
pub(crate) const ICON_TEXELS_F32: f32 = baked::ICON_TEXELS as f32;

/// Атлас, запечённый при сборке.
pub fn sdf_atlas() -> &'static SdfAtlas {
    &ATLAS
}

/// Область атласа по прямоугольнику ячейки в текселях.
pub(crate) fn region_of_cell(x: u32, y: u32, width: u32, height: u32) -> AtlasRegion {
    let atlas_width = baked::WIDTH_TEXELS as f32;
    let atlas_height = baked::HEIGHT_TEXELS as f32;
    AtlasRegion {
        uv_min: [x as f32 / atlas_width, y as f32 / atlas_height],
        uv_max: [(x + width) as f32 / atlas_width, (y + height) as f32 / atlas_height],
    }
}
