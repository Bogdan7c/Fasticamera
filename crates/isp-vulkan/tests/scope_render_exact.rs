//! Показ приборов с отбором примитивов сетки по плитке против ЗАМОРОЖЕННОЙ копии прежнего шейдера
//! (`scope_render_reference.comp`): картинки совпадают БИТ В БИТ (решение 213: точная правка).
//!
//! Замороженный шейдер умеет рисовать только секциями, поэтому оба шейдера получают приборы с ЯВНО выставленным
//! прежним способом рисования ([`PaintOverride::plane_sections`]), а не тем, что называет правило (наложение и
//! цвет по положению — новые ветки продуктового шейдера, их сверяет `scope_picture.rs` с эталоном `scope-model`).
//!
//! Отбор пропускает только примитивы, покрытие которых во всей плитке группы равно нулю, а `max` покрытий от
//! нулей не меняется — поэтому равенство точное, а не «в допуске». Размеры картинки выбраны так, чтобы были и
//! неполные плитки на краях (137×93 не кратно группе 16×8), и размер панели настоящего экрана (540×743).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod scope_support;

use gpu_vulkan::source_window::HorizontalMirror;
use gpu_vulkan::storage_image::StorageFormatPolicy;
use isp_vulkan::scopes::{ColorScopes, ColorScopesConfig, ScopeFrame};
use isp_vulkan::shaders::blobs;
use scope_support::{
    PaintOverride, assert_no_validation_errors, bars75_scene, gpu_context_or_skip, picture_size, random_scene,
    rule_cases, run_scope_frame, small_settings, upload_scene,
};

#[test]
fn tile_culled_render_equals_the_frozen_reference_shader_bit_for_bit() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = small_settings();
    let config = ColorScopesConfig {
        frame_slots: 1,
        counter_sizing: settings.clone(),
    };
    let mut current = ColorScopes::new(&context, &config).unwrap();
    let mut reference =
        ColorScopes::with_render_shader_for_tests(&context, &config, blobs::SCOPE_RENDER_REFERENCE_COMP).unwrap();
    let mut compared_pixels = 0_usize;
    let mut drawn_pixels = 0_usize;
    for scene in [bars75_scene(), random_scene(0x5EED_1234)] {
        let input = upload_scene(&context, &scene, StorageFormatPolicy::PreferTenBitUnorm, (3, 2));
        for (width, height) in [(160, 96), (137, 93), (540, 743)] {
            for (rule_name, resolved) in rule_cases(&settings) {
                let rule = PaintOverride::plane_sections(&resolved, &settings);
                let frame = ScopeFrame {
                    input: input.input(HorizontalMirror::Off),
                    rule: &rule,
                    settings: &settings,
                    picture_size: picture_size(width, height),
                    frame_slot: 0,
                };
                run_scope_frame(&context, &mut current, &frame).unwrap();
                run_scope_frame(&context, &mut reference, &frame).unwrap();
                let new_picture = current.read_back_picture_blocking().unwrap();
                let old_picture = reference.read_back_picture_blocking().unwrap();
                assert_eq!(new_picture.size, old_picture.size);
                for (index, (new_pixel, old_pixel)) in new_picture.texels.iter().zip(&old_picture.texels).enumerate() {
                    let new_bits = new_pixel.map(f32::to_bits);
                    let old_bits = old_pixel.map(f32::to_bits);
                    assert_eq!(
                        new_bits, old_bits,
                        "{} / {rule_name} / {width}x{height}: пиксель {index}",
                        scene.name
                    );
                }
                compared_pixels += new_picture.texels.len();
                drawn_pixels += new_picture.texels.iter().filter(|pixel| pixel[3] > 0.0).count();
            }
        }
    }
    // Сравнение не пустое: сетка и след действительно нарисованы (иначе равенство ничего не доказывало бы).
    assert!(compared_pixels > 1_000_000, "{compared_pixels}");
    assert!(drawn_pixels > 50_000, "{drawn_pixels}");
    assert_no_validation_errors(&context);
}
