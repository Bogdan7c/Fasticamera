//! Вид элементов: «элемент → что рисовать» (решение 84, правило второго использования).
//!
//! Образец — стенд стекла `glass_showcase_build.rs`: там примитивы собираются руками. Здесь тот же
//! приём обобщён в библиотечные строители, чтобы экран камеры (`camera-screen`) собирался только
//! из `ui-kit` и не рисовал примитивы сам. Строители не знают камеры и ничего не считают сами:
//! геометрию дают размещения узлов сцены ([`ui_draw::scene_bridge::NodePlacementPx`]), состояние —
//! снимки элементов, числа — токены.
//!
//! # Порядок рисования
//! Явный и общий для всех строителей: сначала **не стекло под стеклом**, затем **группы стекла**,
//! затем **содержимое поверх** (подписи, иконки, диск затвора). Каждый строитель возвращает свои
//! элементы уже в этом порядке; экран склеивает результаты строителей в нужной последовательности.
//!
//! # Поворот
//! Иконки и подписи ставятся в систему координат своего хоста (члена группы или узла), поэтому
//! поворот узла по ориентации телефона автоматически поворачивает и содержимое.
//!
//! # Ошибки
//! Ничего не обрезается молча: переполнение группы стекла, символ вне запечённого набора и
//! некорректная прозрачность — [`ViewError`].

mod arc_scale;
mod banner;
mod carousel;
mod content;
mod disc_pad;
mod error;
mod focus;
mod glass_surface;
mod panel;
mod ring_picture;
mod ruler;
mod scale;
mod scale_ticks;
mod shutter;
mod surface_material;

pub use arc_scale::{ArcLabel, ArcScaleDrawing, arc_scale_items};
pub use banner::{banner_items, measure_banner_pt};
pub use carousel::{CarouselLabels, carousel_items, carousel_items_in_window};
pub use content::{
    ButtonContent, LabelPainter, button_content_item, centered_icon, centered_text, opacity_of_placement,
};
pub use disc_pad::disc_pad_items;
pub use error::ViewError;
pub use focus::focus_frame_items;
pub use glass_surface::{GlassGroupBuilder, GlassShape, MemberIndex, member_transform};
pub use panel::{panel_glass_group, panel_glass_group_hosted, panel_material_hosted};
pub use ring_picture::{RingGeometry, RingPictureError, ring_picture};
pub use ruler::{MajorTickPeriod, RulerMarkerStyle, ruler_items, ruler_marker_item};
pub use scale::PxPerPt;
pub use scale_ticks::scale_items;
pub use shutter::shutter_items;
pub use surface_material::{MemberSite, single_member_item, surface_member};
