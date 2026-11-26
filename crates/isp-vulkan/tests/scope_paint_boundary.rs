//! Граница «способ рисования следа» у `ColorScopes::record`: отказы типизированными ошибками ДО команд и штатная
//! запись, когда прибор честно называет способ.
//!
//! * прибор просит «наложение» при числе плоскостей не три -> `PaintPlaneCountMismatch`;
//! * прибор просит «по положению» при числе плоскостей не одна -> `PaintPlaneCountMismatch`;
//! * секции с числом цветов не по числу плоскостей -> `TraceColorCountMismatch`;
//! * после отказа ничего не записано (счётчики кадров прежние), следующий честный кадр проходит;
//! * штатные способы (секции, наложение на трёх плоскостях, «по положению» на одной) записываются.

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
use scope_model::{ResolvedScope, TracePaint, TraceStyle};
use scope_support::{
    PaintOverride, assert_no_validation_errors, bars75_scene, gpu_context_or_skip, picture_size, rule_cases,
    run_scope_frame, small_settings, upload_scene,
};

/// Способ рисования, который сам прибор называет по умолчанию (как в продукте).
fn own_style(scope: &ResolvedScope) -> TraceStyle {
    scope.trace_style(&small_settings().render)
}

#[test]
fn a_paint_that_does_not_fit_the_grid_is_refused_before_any_command_and_honest_paints_are_recorded() {
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
    let find = |name: &str| &cases.iter().find(|(case, _)| *case == name).unwrap().1;
    let (waveform, parade, vectorscope) = (find("waveform"), find("parade"), find("vectorscope-ycbcr"));

    let try_style = |scopes: &mut ColorScopes, scope: &ResolvedScope, style: TraceStyle| {
        let rule = PaintOverride { inner: scope, style };
        let frame = ScopeFrame {
            input: input.input(HorizontalMirror::Off),
            rule: &rule,
            settings: &settings,
            picture_size: picture_size(64, 48),
            frame_slot: 0,
        };
        run_scope_frame(&context, scopes, &frame).map(|_| ())
    };
    let overlaid = TraceStyle::PlanesOverlaid {
        primaries: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    };

    let before = scopes.pass_counters();
    // «Наложение» требует три плоскости, а у вектороскопа одна.
    assert_eq!(
        try_style(&mut scopes, vectorscope, overlaid.clone()),
        Err(ScopeError::PaintPlaneCountMismatch {
            paint: TracePaint::PlanesOverlaid,
            required: 3,
            planes: 1
        })
    );
    // «По положению» требует одну плоскость, а у сетки parade их три.
    assert_eq!(
        try_style(&mut scopes, parade, own_style(vectorscope)),
        Err(ScopeError::PaintPlaneCountMismatch {
            paint: TracePaint::ColorByPosition,
            required: 1,
            planes: 3
        })
    );
    // Секции с двумя цветами на трёх плоскостях: цветов не хватает.
    assert_eq!(
        try_style(
            &mut scopes,
            parade,
            TraceStyle::PlaneSections {
                colors: vec![[1.0; 3]; 2]
            }
        ),
        Err(ScopeError::TraceColorCountMismatch { colors: 2, planes: 3 })
    );
    assert_eq!(scopes.pass_counters(), before, "отказ ничего не записал");

    // Честные кадры: наложение на трёх плоскостях, «по положению» на одной, секции на трёх.
    for (scope, style, what) in [
        (waveform, overlaid, "наложение"),
        (vectorscope, own_style(vectorscope), "по положению"),
        (parade, own_style(parade), "секции"),
    ] {
        let recorded_before = scopes.pass_counters().recorded;
        assert_eq!(try_style(&mut scopes, scope, style), Ok(()), "{what}");
        assert_eq!(scopes.pass_counters().recorded, recorded_before + 1, "{what}");
    }
    assert_no_validation_errors(&context);
}
