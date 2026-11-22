//! Вход и выход одного кадра: что рисовать, поверх какого фона, и что из этого вышло.

use gpu_vulkan::backdrop::BackdropSource;
use gpu_vulkan::test_pattern::SurfaceTransform;
use ui_draw::color::DisplayColor;
use ui_draw::draw_list::DrawList;
use ui_draw::geometry::{PxRect, PxSize};

/// Где картинка фона лежит на ЛОГИЧЕСКОМ экране — так, как её разместил композитор (letterbox, масштаб).
/// Вне прямоугольника фон чёрный: стекло видит то же, что видит глаз под ним.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BackdropPlacement {
    /// Прямоугольник картинки фона, логические px.
    pub destination: PxRect,
}

/// Что лежит под стеклом.
#[derive(Debug, Clone, Copy)]
pub enum BackdropInput<'a> {
    /// Живой фон (кадр видоискателя).
    Live {
        /// Нейтральная подложка (`gpu-vulkan::backdrop`).
        source: &'a BackdropSource,
        /// Как фон размещён на экране.
        placement: BackdropPlacement,
    },
    /// Фона нет (камера не готова): стекло лежит над сплошным цветом.
    Absent {
        /// Цвет вместо фона (закодированные значения экрана).
        fill: DisplayColor,
    },
}

/// Один кадр UI.
#[derive(Debug, Clone, Copy)]
pub struct UiFrame<'a> {
    /// Что рисовать (в логических px, портрет).
    pub draw_list: &'a DrawList,
    /// Логический размер экрана, px (портрет, до поворота цели).
    pub logical_size: PxSize,
    /// Поворот цели показа относительно прямого вида.
    pub surface_transform: SurfaceTransform,
    /// Фон под стеклом.
    pub backdrop: BackdropInput<'a>,
}

/// Что случилось с пирамидой размытия в этом кадре.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PyramidOutcome {
    /// В кадре нет стекла: пирамида не нужна и не трогалась.
    NotNeeded,
    /// Ключ фона изменился: барьер чтения фона и ступени записаны заново.
    Rebuilt,
    /// Ключ тот же: ступени прошлого кадра переиспользованы (путь Recompose).
    Reused,
}

/// Итог записи кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiFrameReport {
    /// Судьба пирамиды.
    pub pyramid: PyramidOutcome,
    /// Вызовов рисования в проходе UI.
    pub draw_calls: u32,
    /// Фигур в кадре.
    pub shape_instances: u32,
    /// Глифов и иконок в кадре.
    pub glyph_instances: u32,
    /// Групп стекла в кадре.
    pub glass_groups: u32,
    /// Элементов-картинок в кадре.
    pub image_instances: u32,
    /// Картинок, загруженных в видеопамять в ЭТОМ кадре (копия записана в его команды).
    pub picture_uploads: u32,
    /// Картинок кадра, найденных в кэше (загрузки не было).
    pub picture_cache_hits: u32,
}

/// Точки кадра для замера GPU и диагностики: вызывающий пишет в них метки времени.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameMarker {
    /// Перед первой командой кадра.
    Start,
    /// После пирамиды (или сразу, если её не было), до прохода UI.
    PyramidDone,
    /// После прохода UI.
    UiPassDone,
}
