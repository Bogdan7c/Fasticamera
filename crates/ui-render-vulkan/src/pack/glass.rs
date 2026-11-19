//! Группа стекла и её члены (`GlassGroup` / `GlassMember` в `ui_glass_data.glsl`).
//! Группа — 9 × 16 = 144 байта, член — 3 × 16 = 48 байт (std430).

use ui_draw::draw_list::{DrawListError, GlassGroupItem};
use ui_draw::geometry::PxRect;
use ui_draw::glass::EdgeProfile;

use super::shape::geometry_parameters;
use super::{put_affine_rows, put_floats, put_ints, put_rect};
use crate::error::UiRenderError;

/// Размер группы в SSBO.
pub(crate) const GLASS_GROUP_BYTES: usize = 144;
/// Размер члена в SSBO.
pub(crate) const GLASS_MEMBER_BYTES: usize = 48;

/// Дописывает группу в `groups` и её членов в `members`. `first_member` — номер первого члена группы
/// в общем массиве членов кадра.
pub(crate) fn pack_glass_group(
    item_index: usize,
    group: &GlassGroupItem,
    bounds: &PxRect,
    first_member: usize,
    groups: &mut Vec<u8>,
    members: &mut Vec<u8>,
) -> Result<(), UiRenderError> {
    for member in &group.members {
        let inverse = member
            .transform
            .inverse()
            .map_err(|source| DrawListError::DegenerateTransform {
                item: item_index,
                source,
            })?;
        put_affine_rows(members, &inverse);
        put_floats(members, &geometry_parameters(&member.geometry));
    }
    let params = &group.params;
    let profile = match params.edge_profile {
        EdgeProfile::QuarterCircle => 0.0,
        EdgeProfile::ConvexSuperellipse => 1.0,
    };
    // Правило нажатия группы (максимальная сила, при равенстве — первый член) живёт в эталоне.
    let press = group.dominant_press();
    let start = groups.len();
    put_rect(groups, bounds);
    put_floats(
        groups,
        &[
            params.lens_zone_px,
            params.refraction_strength_px,
            params.blur_radius_px,
            params.saturation,
        ],
    );
    put_floats(
        groups,
        &[
            params.dispersion,
            params.specular.alpha,
            params.specular.rim_width_px,
            params.specular.exponent,
        ],
    );
    put_floats(
        groups,
        &[
            params.dark_edge.strength,
            params.dark_edge.width_px,
            params.dimming.max_alpha,
            params.dimming.luminance_low,
        ],
    );
    put_floats(
        groups,
        &[
            params.dimming.luminance_high,
            params.tint_level,
            params.tint_alpha_at_full,
            profile,
        ],
    );
    put_floats(
        groups,
        &[
            params.shadow.radius_px,
            params.shadow.alpha,
            params.shadow.offset_px,
            params.press.refraction_boost,
        ],
    );
    put_floats(
        groups,
        &[
            params.press.glow_radius_px,
            params.press.glow_alpha,
            group.merge_radius_px,
            group.effect,
        ],
    );
    put_floats(
        groups,
        &[
            group.opacity.get(),
            press.amount,
            press.touch_point_px.x,
            press.touch_point_px.y,
        ],
    );
    // Индексы членов — в пределах буфера (limits × 16), `i32` хватает с огромным запасом.
    put_ints(groups, &[first_member as i32, group.members.len() as i32, 0, 0]);
    debug_assert_eq!(groups.len() - start, GLASS_GROUP_BYTES);
    Ok(())
}
