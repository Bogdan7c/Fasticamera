//! K1 «Стеклянная поверхность»: строитель группы стекла.
//!
//! Группа рисуется одним проходом над одним снимком фона; её члены сливаются гладким минимумом с
//! радиусом слияния. Строитель берёт на себя три вещи, которые экран иначе делал бы руками:
//! форму члена по размещению узла, запекание масштаба нажатия в преобразование члена (рендер его
//! не добавляет) и предел числа членов с явной ошибкой при переполнении.

use ui_draw::color::Opacity;
use ui_draw::draw_list::{GlassGroupItem, GlassMember, MAX_GLASS_GROUP_MEMBERS, PressState};
use ui_draw::geometry::PxSize;
use ui_draw::glass::GlassStyle;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::shape::{CornerSpec, ShapeGeometry};
use ui_draw::transform::Affine2;

use super::error::ViewError;
use super::scale::PxPerPt;

/// Форма члена группы; размер берётся из размещения узла.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GlassShape {
    /// Капсула: радиус — половина меньшей стороны (форма стекла по умолчанию у Apple).
    Capsule,
    /// Круг: диаметр — меньшая сторона размещения.
    Circle,
    /// Скруглённый прямоугольник с непрерывным скруглением iOS.
    RoundedRect { corner_radius_pt: f32 },
}

/// Номер члена в группе в порядке добавления.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemberIndex(usize);

impl MemberIndex {
    pub fn get(self) -> usize {
        self.0
    }
}

/// Член до сборки: параметры стекла зависят от размера группы, поэтому собирается в [`GlassGroupBuilder::build`].
#[derive(Debug, Clone, Copy)]
struct PendingMember {
    geometry: ShapeGeometry,
    placement: NodePlacementPx,
    press: PressState,
}

/// Строитель группы стекла.
#[derive(Debug, Clone)]
pub struct GlassGroupBuilder<'a> {
    style: &'a GlassStyle,
    scale: PxPerPt,
    merge_radius_pt: f32,
    members: Vec<PendingMember>,
}

impl<'a> GlassGroupBuilder<'a> {
    /// Группа без слияния. Пресет выбирает экран по роли (`tokens::GlassTokens::style_for`).
    pub fn new(style: &'a GlassStyle, scale: PxPerPt) -> Self {
        Self {
            style,
            scale,
            merge_radius_pt: 0.0,
            members: Vec::new(),
        }
    }

    /// Радиус слияния членов (у Apple — `spacing` контейнера), pt. 0 — формы не сливаются.
    pub fn with_merge_radius_pt(mut self, merge_radius_pt: f32) -> Self {
        self.merge_radius_pt = merge_radius_pt;
        self
    }

    /// Добавить член. Предел [`MAX_GLASS_GROUP_MEMBERS`] — ошибка, а не молчаливая обрезка.
    pub fn add_member(
        &mut self,
        shape: GlassShape,
        placement: NodePlacementPx,
        press: PressState,
    ) -> Result<MemberIndex, ViewError> {
        if self.members.len() >= MAX_GLASS_GROUP_MEMBERS {
            return Err(ViewError::GlassGroupFull {
                max: MAX_GLASS_GROUP_MEMBERS,
            });
        }
        let geometry = member_geometry(shape, placement.size, self.scale);
        self.members.push(PendingMember {
            geometry,
            placement,
            press,
        });
        Ok(MemberIndex(self.members.len() - 1))
    }

    /// Собрать группу. Параметры материала считаются по размеру **первого** члена с учётом его
    /// масштаба на экране (раскрывающаяся панель не получает линзу большой формы на маленькой).
    pub fn build(self, effect: f32, opacity: Opacity) -> Result<GlassGroupItem, ViewError> {
        if !effect.is_finite() || !(0.0..=1.0).contains(&effect) {
            return Err(ViewError::InvalidEffect(effect));
        }
        let first = self.members.first().ok_or(ViewError::EmptyGlassGroup)?;
        let params = self
            .style
            .resolve(effective_size_px(&first.placement), self.scale.get());
        let members = self
            .members
            .iter()
            .map(|pending| GlassMember {
                geometry: pending.geometry,
                // Масштаб нажатия запекается здесь: рендер его не добавляет.
                transform: Affine2::uniform_scale(params.press.scale_for(pending.press.amount))
                    .then(pending.placement.transform),
                press: pending.press,
            })
            .collect();
        Ok(GlassGroupItem {
            params,
            members,
            merge_radius_px: self.merge_radius_pt * self.scale.get(),
            effect,
            opacity,
        })
    }
}

/// Геометрия члена по форме и размеру размещения. Общая для стекла и поверхности: у обоих одна форма
/// на одном и том же месте, меняется только материал.
pub(super) fn member_geometry(shape: GlassShape, size: PxSize, scale: PxPerPt) -> ShapeGeometry {
    match shape {
        GlassShape::Capsule => ShapeGeometry::Capsule { size },
        GlassShape::Circle => ShapeGeometry::Circle {
            diameter: size.width.min(size.height),
        },
        GlassShape::RoundedRect { corner_radius_pt } => ShapeGeometry::RoundedRect {
            size,
            corner: CornerSpec::continuous(corner_radius_pt * scale.get()),
        },
    }
}

/// Размер элемента на экране в px с учётом масштаба его преобразования (раскрытие панели).
fn effective_size_px(placement: &NodePlacementPx) -> PxSize {
    let transform = &placement.transform;
    // Образы осей x и y в строковой записи `Affine2`: (a, c) и (b, d).
    let scale_x = transform.a.hypot(transform.c);
    let scale_y = transform.b.hypot(transform.d);
    PxSize::new(placement.size.width * scale_x, placement.size.height * scale_y)
}

/// Преобразование члена группы (с масштабом нажатия) — в нём ставится содержимое члена.
pub fn member_transform(group: &GlassGroupItem, index: MemberIndex) -> Result<Affine2, ViewError> {
    group
        .members
        .get(index.get())
        .map(|member| member.transform)
        .ok_or(ViewError::MemberOutOfRange {
            index: index.get(),
            len: group.members.len(),
        })
}
