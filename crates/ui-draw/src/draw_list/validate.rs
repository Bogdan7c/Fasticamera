//! Проверка списка рисования перед отправкой рендеру. Один проход, без побочных эффектов.

use crate::color::DisplayColor;
use crate::draw_list::DrawList;
use crate::draw_list::error::DrawListError;
use crate::draw_list::glass_group::{GlassGroupItem, GlassMember, MAX_GLASS_GROUP_MEMBERS};
use crate::draw_list::items::{DrawItem, IconItem, ImageItem, ShapeItem, TextItem};
use crate::glass::GlassParamsPx;
use crate::transform::Affine2;

impl DrawList {
    /// Проверить все элементы: числа конечны, размеры неотрицательны, преобразования обратимы,
    /// группы стекла не пусты и не больше [`MAX_GLASS_GROUP_MEMBERS`]. Возвращает первую найденную ошибку.
    pub fn validate(&self) -> Result<(), DrawListError> {
        for (index, item) in self.items.iter().enumerate() {
            match item {
                DrawItem::Shape(shape) => validate_shape(index, shape)?,
                DrawItem::Glass(group) => validate_glass_group(index, group)?,
                DrawItem::Text(text) => validate_text(index, text)?,
                DrawItem::Icon(icon) => validate_icon(index, icon)?,
                DrawItem::Image(image) => validate_image(index, image)?,
            }
        }
        Ok(())
    }
}

fn check_transform(item: usize, transform: &Affine2) -> Result<(), DrawListError> {
    transform
        .check_invertible()
        .map_err(|source| DrawListError::DegenerateTransform { item, source })
}

fn check_finite(item: usize, field: &'static str, value: f32) -> Result<(), DrawListError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(DrawListError::NotFinite { item, field })
    }
}

fn check_non_negative(item: usize, field: &'static str, value: f32) -> Result<(), DrawListError> {
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        Err(DrawListError::InvalidSize { item, field, value })
    }
}

fn check_unit(item: usize, field: &'static str, value: f32) -> Result<(), DrawListError> {
    check_finite(item, field, value)?;
    if (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(DrawListError::OutOfUnitRange { item, field, value })
    }
}

fn check_color(item: usize, field: &'static str, color: DisplayColor) -> Result<(), DrawListError> {
    for value in [color.r, color.g, color.b, color.a] {
        check_unit(item, field, value)?;
    }
    Ok(())
}

fn validate_shape(item: usize, shape: &ShapeItem) -> Result<(), DrawListError> {
    shape
        .geometry
        .validate()
        .map_err(|source| DrawListError::InvalidShape { item, source })?;
    check_transform(item, &shape.transform)?;
    check_color(item, "fill", shape.fill)?;
    if let Some(stroke) = shape.stroke {
        check_non_negative(item, "stroke.width_px", stroke.width_px)?;
        check_color(item, "stroke.color", stroke.color)?;
    }
    if let Some(shadow) = shape.shadow {
        check_non_negative(item, "shadow.radius_px", shadow.radius_px)?;
        check_non_negative(item, "shadow.offset_px", shadow.offset_px)?;
        check_unit(item, "shadow.alpha", shadow.alpha)?;
    }
    Ok(())
}

fn validate_text(item: usize, text: &TextItem) -> Result<(), DrawListError> {
    check_transform(item, &text.transform)?;
    check_color(item, "color", text.color)?;
    check_non_negative(item, "run.advance_px", text.run.advance_px)?;
    check_non_negative(item, "run.cap_height_px", text.run.cap_height_px)
}

fn validate_icon(item: usize, icon: &IconItem) -> Result<(), DrawListError> {
    check_transform(item, &icon.transform)?;
    check_color(item, "color", icon.color)?;
    // Иконка нулевого размера бессмысленна: рамка, в которую вписывается контур, схлопнута.
    check_non_negative(item, "size_px.width", icon.size_px.width)?;
    check_non_negative(item, "size_px.height", icon.size_px.height)?;
    if icon.size_px.width == 0.0 || icon.size_px.height == 0.0 {
        return Err(DrawListError::InvalidSize {
            item,
            field: "size_px",
            value: 0.0,
        });
    }
    Ok(())
}

fn validate_glass_group(item: usize, group: &GlassGroupItem) -> Result<(), DrawListError> {
    if group.members.is_empty() {
        return Err(DrawListError::EmptyGlassGroup { item });
    }
    if group.members.len() > MAX_GLASS_GROUP_MEMBERS {
        return Err(DrawListError::too_many_members(item, group.members.len()));
    }
    check_non_negative(item, "merge_radius_px", group.merge_radius_px)?;
    check_unit(item, "effect", group.effect)?;
    validate_glass_params(item, &group.params)?;
    group
        .members
        .iter()
        .try_for_each(|member| validate_glass_member(item, member))
}

fn validate_glass_member(item: usize, member: &GlassMember) -> Result<(), DrawListError> {
    member
        .geometry
        .validate()
        .map_err(|source| DrawListError::InvalidShape { item, source })?;
    check_transform(item, &member.transform)?;
    check_unit(item, "press.amount", member.press.amount)?;
    check_finite(item, "press.touch_point_local.x", member.press.touch_point_local.x)?;
    check_finite(item, "press.touch_point_local.y", member.press.touch_point_local.y)
}

/// Параметры стекла, посчитанные `resolve`, должны быть конечны и неотрицательны там, где это нужно:
/// нечисловое значение означает, что `resolve` вызвали с `px_per_pt ≤ 0` или невалидным стилем.
fn validate_glass_params(item: usize, params: &GlassParamsPx) -> Result<(), DrawListError> {
    let non_negative: [(&'static str, f32); 10] = [
        ("params.lens_zone_px", params.lens_zone_px),
        ("params.refraction_strength_px", params.refraction_strength_px),
        ("params.blur_radius_px", params.blur_radius_px),
        ("params.saturation", params.saturation),
        ("params.dispersion", params.dispersion),
        ("params.specular.rim_width_px", params.specular.rim_width_px),
        ("params.dark_edge.width_px", params.dark_edge.width_px),
        ("params.shadow.radius_px", params.shadow.radius_px),
        ("params.shadow.offset_px", params.shadow.offset_px),
        ("params.press.glow_radius_px", params.press.glow_radius_px),
    ];
    for (field, value) in non_negative {
        check_non_negative(item, field, value)?;
    }
    check_unit(item, "params.tint_level", params.tint_level)?;
    check_unit(item, "params.dimming.max_alpha", params.dimming.max_alpha)
}

fn validate_image(item: usize, image: &ImageItem) -> Result<(), DrawListError> {
    check_transform(item, &image.transform)?;
    // Пиксели проверены при сборке `UiPicture` (размер ≠ 0, длина буфера); здесь — только рамка.
    // Рамка нулевого размера бессмысленна: картинке некуда лечь.
    check_non_negative(item, "size_px.width", image.size_px.width)?;
    check_non_negative(item, "size_px.height", image.size_px.height)?;
    if image.size_px.width == 0.0 || image.size_px.height == 0.0 {
        return Err(DrawListError::InvalidSize {
            item,
            field: "size_px",
            value: 0.0,
        });
    }
    Ok(())
}
