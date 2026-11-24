//! Абляции СВОЙСТВ стекла для стенда насыщения (сессия 16a, пакет G3): что из цены стекла чем занято.
//!
//! # Зачем (простыми словами)
//! Стекло — главная цена показа, но неизвестно, какая его часть дорога: растеризация и смешивание, поле формы,
//! выборка размытого фона, средняя яркость группы, преломление или тень. Абляция берёт настоящий список
//! рисования экрана и меняет ТОЛЬКО параметры стиля групп стекла (это сцена стенда, продукт не затрагивается):
//! шейдер остаётся тем же, но часть его работы пропускается быстрыми путями или вырождается. Разность цен
//! работ — цена выключенной части.
//!
//! Модуль чистый (GPU не нужен): [`GlassSubset`] выбирает группы, [`GlassStyleAblation`] меняет их параметры.

use serde::Serialize;
use ui_draw::color::Opacity;
use ui_draw::draw_list::{DrawItem, DrawList, GlassGroupItem};

/// Радиус размытия, при котором уровень пирамиды равен 0 (`log2(radius / 1,5) = 0`, см. `blur_lod` в шейдере).
/// Уровень k даёт радиус `1,5 · 2^k`.
const BLUR_RADIUS_AT_LEVEL_ZERO_PX: f32 = 1.5;

/// Какие группы стекла остаются в кадре.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GlassSubset {
    /// Все группы.
    All,
    /// Только самая большая по площади области рисования (в CALIBRATE это панель ручек на пол-экрана).
    LargestOnly,
    /// Все, кроме самой большой.
    WithoutLargest,
    /// Только самая маленькая группа (кнопка): почти нулевая площадь, остаётся постоянная цена прохода и пирамиды.
    SmallestOnly,
    /// Ни одной группы: цена всего, что не зависит от групп (компоновщик, пирамида, начало и конец прохода).
    NoGroups,
}

/// Какое свойство стекла выключается (стиль каждой группы).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GlassStyleAblation {
    /// Продуктовый стиль.
    Product,
    /// То же, что `Invisible`, но и без тени: квад только по форме, одно поле формы на пиксель.
    InvisibleNoShadow,
    /// Прозрачность группы 0: растеризация, два поля формы (слой и тень) и смешивание, без выборок фона.
    Invisible,
    /// Без плёнки тона и затемнения: средняя яркость группы (5 выборок) не нужна.
    NoToneAndDimming,
    /// Без преломления: все выборки фона в одной точке (одна вместо трёх на краях).
    NoRefraction,
    /// Радиус размытия округлён до целого уровня пирамиды: одна выборка фона вместо двух.
    WholeBlurLevel,
    /// Без тени.
    NoShadow,
    /// Без блика, тёмного края и свечения нажатия, насыщенность 1.
    NoFinishing,
    /// Пол цены: растеризация, смешивание и поле формы + одна выборка размытого фона (целый уровень).
    Floor,
    /// Пол цены с резким фоном: одна выборка полноразмерной картинки вместо пирамиды.
    FloorSharp,
}

impl GlassStyleAblation {
    /// Меняет параметры одной группы по абляции.
    pub fn apply_to(self, group: &mut GlassGroupItem) {
        match self {
            Self::Product => {}
            Self::Invisible => group.opacity = Opacity::TRANSPARENT,
            Self::InvisibleNoShadow => {
                group.opacity = Opacity::TRANSPARENT;
                group.params.shadow.alpha = 0.0;
            }
            Self::NoToneAndDimming => {
                group.params.tint_alpha_at_full = 0.0;
                group.params.dimming.max_alpha = 0.0;
            }
            Self::NoRefraction => group.params.refraction_strength_px = 0.0,
            Self::WholeBlurLevel => whole_blur_level(group),
            Self::NoShadow => group.params.shadow.alpha = 0.0,
            Self::NoFinishing => no_finishing(group),
            Self::Floor => {
                no_everything_but_field(group);
                whole_blur_level(group);
            }
            Self::FloorSharp => {
                no_everything_but_field(group);
                group.params.blur_radius_px = 0.0;
            }
        }
    }
}

/// Округляет уровень размытия до ближайшего целого (радиус `1,5 · 2^k`); нулевой радиус остаётся резким.
fn whole_blur_level(group: &mut GlassGroupItem) {
    let radius = group.params.blur_radius_px;
    if radius <= 0.0 {
        return;
    }
    let level = (radius.max(BLUR_RADIUS_AT_LEVEL_ZERO_PX) / BLUR_RADIUS_AT_LEVEL_ZERO_PX)
        .log2()
        .round();
    group.params.blur_radius_px = BLUR_RADIUS_AT_LEVEL_ZERO_PX * level.exp2();
}

/// Без блика, тёмного края, свечения нажатия и усиления насыщенности.
fn no_finishing(group: &mut GlassGroupItem) {
    group.params.specular.alpha = 0.0;
    group.params.dark_edge.strength = 0.0;
    group.params.press.glow_alpha = 0.0;
    group.params.saturation = 1.0;
}

/// Остаётся только поле формы, выборка фона и смешивание.
fn no_everything_but_field(group: &mut GlassGroupItem) {
    no_finishing(group);
    group.params.refraction_strength_px = 0.0;
    group.params.dispersion = 0.0;
    group.params.tint_alpha_at_full = 0.0;
    group.params.dimming.max_alpha = 0.0;
    group.params.shadow.alpha = 0.0;
}

/// Площадь области рисования группы, px² (0 — группа пуста или вырождена).
fn draw_area_px(group: &GlassGroupItem) -> f32 {
    group
        .draw_rect_px()
        .map_or(0.0, |rect| rect.size.width * rect.size.height)
}

/// Номер группы с наибольшей (`prefer_larger`) или наименьшей площадью среди элементов списка (`None` — стекла
/// нет). Среди равных — первая.
fn extreme_group_index(list: &DrawList, prefer_larger: bool) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (index, item) in list.items.iter().enumerate() {
        if let DrawItem::Glass(group) = item {
            let area = draw_area_px(group);
            let better = |best_area: f32| {
                if prefer_larger {
                    area > best_area
                } else {
                    area < best_area
                }
            };
            if best.is_none_or(|(_, best_area)| better(best_area)) {
                best = Some((index, area));
            }
        }
    }
    best.map(|(index, _)| index)
}

/// Список только из стекла: группы выбраны `subset`, стиль изменён `style`. Порядок групп сохраняется.
pub fn apply_glass_ablation(list: &DrawList, subset: GlassSubset, style: GlassStyleAblation) -> DrawList {
    let largest = extreme_group_index(list, true);
    let smallest = extreme_group_index(list, false);
    let items = list
        .items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let DrawItem::Glass(group) = item else { return None };
            let is_largest = Some(index) == largest;
            let kept = match subset {
                GlassSubset::All => true,
                GlassSubset::LargestOnly => is_largest,
                GlassSubset::WithoutLargest => !is_largest,
                GlassSubset::SmallestOnly => Some(index) == smallest,
                GlassSubset::NoGroups => false,
            };
            kept.then(|| {
                let mut changed = group.clone();
                style.apply_to(&mut changed);
                DrawItem::Glass(changed)
            })
        })
        .collect();
    DrawList {
        items,
        light: list.light,
    }
}
