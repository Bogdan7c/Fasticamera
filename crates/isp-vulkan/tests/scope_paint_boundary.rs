//! Граница «способ рисования следа» у `ColorScopes::record`: отказы типизированными ошибками ДО команд и штатная
//! запись, когда прибор честно называет способ.
//!
//! * прибор просит «по положению», но таблицы цветов не отдал -> `MissingPositionPalette`;
//! * прибор просит «наложение» при числе плоскостей не три -> `PaintPlaneCountMismatch`;
//! * после отказа ничего не записано (счётчики кадров прежние), следующий честный кадр проходит;
//! * штатный способ (секции) записывается и меняет счётчики записи.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod scope_support;

use gpu_vulkan::source_window::HorizontalMirror;
use gpu_vulkan::storage_image::StorageFormatPolicy;
use isp_vulkan::scopes::{ColorScopes, ColorScopesConfig, ScopeError, ScopeFrame};
use scope_model::TracePaint;
use scope_support::{
    PaintOverride, assert_no_validation_errors, bars75_scene, gpu_context_or_skip, picture_size, rule_cases,
    run_scope_frame, small_settings, upload_scene,
};

#[test]
fn a_paint_without_what_it_needs_is_refused_before_any_command_and_an_honest_paint_is_recorded() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = small_settings();
    let mut scopes = ColorScopes::new(
        &context,
        &ColorScopesConfig {
            frame_slots: 1,
            counter_sizing: settings.clone(),
        },
    )
    .unwrap();
    let input = upload_scene(
        &context,
        &bars75_scene(),
        StorageFormatPolicy::PreferTenBitUnorm,
        (0, 0),
    );
    let cases = rule_cases(&settings);
    let (_, waveform) = cases.iter().find(|(name, _)| *name == "waveform").unwrap();
    let (_, vectorscope) = cases.iter().find(|(name, _)| *name == "vectorscope-ycbcr").unwrap();

    let try_paint = |scopes: &mut ColorScopes, scope, paint| {
        let rule = PaintOverride {
            inner: scope,
            paint,
            palette: None,
        };
        let frame = ScopeFrame {
            input: input.input(HorizontalMirror::Off),
            rule: &rule,
            settings: &settings,
            picture_size: picture_size(64, 48),
            frame_slot: 0,
        };
        run_scope_frame(&context, scopes, &frame).map(|_| ())
    };

    // Нет таблицы цветов: рисовать «по положению» нечем.
    let before = scopes.pass_counters();
    assert_eq!(
        try_paint(&mut scopes, vectorscope, TracePaint::ColorByPosition),
        Err(ScopeError::MissingPositionPalette)
    );
    // «Наложение» требует три плоскости, а у вектороскопа одна.
    assert_eq!(
        try_paint(&mut scopes, vectorscope, TracePaint::PlanesOverlaid),
        Err(ScopeError::PaintPlaneCountMismatch {
            paint: TracePaint::PlanesOverlaid,
            required: 3,
            planes: 1
        })
    );
    // «По положению» требует одну плоскость, а у parade-сетки три (проверка плоскостей идёт раньше таблицы).
    let (_, parade) = cases.iter().find(|(name, _)| *name == "parade").unwrap();
    assert_eq!(
        try_paint(&mut scopes, parade, TracePaint::ColorByPosition),
        Err(ScopeError::PaintPlaneCountMismatch {
            paint: TracePaint::ColorByPosition,
            required: 1,
            planes: 3
        })
    );
    assert_eq!(scopes.pass_counters(), before, "отказ ничего не записал");

    // Секции годятся любому числу плоскостей: честный кадр записывается.
    assert_eq!(try_paint(&mut scopes, waveform, TracePaint::PlaneSections), Ok(()));
    assert_eq!(scopes.pass_counters().recorded, before.recorded + 1);
    assert_eq!(scopes.pass_counters().render_dispatches, before.render_dispatches + 1);
    assert_no_validation_errors(&context);
}
