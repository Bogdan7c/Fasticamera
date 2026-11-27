//! Эталон основы в рабочем пространстве по кадрированию внутри полезной области (помощник тестов видоискателя).

use super::*;
use isp_model::tone::working_stage::{apply_working_base, working_display_to_output, xyz_d50_to_working_scene};
use isp_model::white_level::{WhiteLevelField, WhiteLevelPlane, apply_white_balance_per_point};

/// Эталон ЛЮБОЙ основы тона с учётом того, где кадрирование лежит в полезной области (виньетирование и белый
/// точки берутся по координатам ПОЛЕЗНОЙ области, как у `reference_upright_in_valid_area`). Для прежней основы —
/// та же функция; для моделей в `W` — цепочка с белым каждой точки и видом картинки (в `run_reference_working` кроп всегда
/// считается целой полезной областью, поэтому её тело повторено здесь).
pub fn reference_upright_encoded_in_valid_area(
    raw: &RawFrame,
    valid_area: PixelRect,
    crop: PixelRect,
    orientation: SensorOrientation,
    params: &IspParams,
) -> UprightPixels {
    // Любая модель в рабочем пространстве (AgX, ACES, filmic) идёт одной цепочкой; прежняя основа — своей.
    let Some(base) = params.tone_base.working_stage() else {
        let legacy = reference_upright_in_valid_area(raw, valid_area, crop, orientation, params);
        return UprightPixels {
            width: legacy.width,
            height: legacy.height,
            pixels: legacy.pixels,
        };
    };
    let sampling = LensShadingSampling::for_demosaic(params.demosaic);
    let normalized = normalize_levels(&raw.mosaic(crop).unwrap(), &params.levels);
    let place = CropInValidArea::at(crop.left - valid_area.left, crop.top - valid_area.top, valid_area.size).unwrap();
    let (shading_corrected, _) =
        apply_lens_shading_sampled(&normalized, &params.lens_shading, place, sampling).unwrap();
    let field = WhiteLevelField::new(&params.lens_shading, &params.white_balance, place, sampling);
    let balanced = apply_white_balance_per_point(&shading_corrected, &params.white_balance, &field);
    let white_levels = WhiteLevelPlane::for_demosaic(&field, params.demosaic, shading_corrected.size());
    let camera_rgb = demosaic(&balanced, params.demosaic);
    let xyz_d50 = camera_rgb_to_xyz_d50(&camera_rgb, params.color.camera_to_xyz_d50());
    let working_scene = xyz_d50_to_working_scene(&xyz_d50, base);
    let working_display = apply_working_base(&working_scene, &white_levels, base).unwrap();
    let working_looked = apply_look(&working_display, &params.look);
    let display_encoded = encode_display(
        &working_display_to_output(&working_looked, base),
        params.output_profile.transfer(),
    );
    upright_pixels(&display_encoded, orientation)
}
