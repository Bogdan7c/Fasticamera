//! Жизненный цикл приборов: смена прибора, осей и размера на ходу без создания пайплайнов; вход не меняется;
//! отказы ДО команд; освобождение слотов; зеркало; пустые отсчёты.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod scope_support;

use ash::vk;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::kit_test_support::run_one_shot_blocking;
use gpu_vulkan::live_objects::LiveObjectKind;
use gpu_vulkan::source_window::{HorizontalMirror, SourceWindow};
use gpu_vulkan::storage_image::StorageFormatPolicy;
use isp_vulkan::scopes::{ColorScopes, ColorScopesConfig, PictureChange, ScopeError, ScopeFrame, ScopeInput};
use scope_model::{SampleStride, ScopeModelSettings, ScopeRule};
use scope_support::{
    Scene, all_scenes, assert_no_validation_errors, bars75_scene, counters_delta, cpu_counters, gpu_context_or_skip,
    narrow_scene, picture_size, random_scene, rule_cases, run_scope_frame, small_settings, upload_scene,
};

fn scopes_for(context: &gpu_vulkan::context::GpuContext, settings: &ScopeModelSettings, slots: usize) -> ColorScopes {
    ColorScopes::new(
        context,
        &ColorScopesConfig {
            frame_slots: slots,
            counter_sizing: settings.clone(),
        },
    )
    .unwrap()
}

#[test]
fn switching_kind_and_axes_between_frames_creates_no_pipelines_and_counts_right() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = small_settings();
    let mut scopes = scopes_for(&context, &settings, 2);
    let pipelines_at_creation = scopes.accumulator_pipeline_count();
    let live_pipelines = context.live_objects().count(LiveObjectKind::Pipeline);
    let input = upload_scene(
        &context,
        &random_scene(11),
        StorageFormatPolicy::PreferTenBitUnorm,
        (5, 3),
    );
    let shown = input.shown_codes(HorizontalMirror::Off);
    let rules = rule_cases(&settings);
    // Порядок нарочно перебивает «соседей»: целочисленный → OKLab → целочисленный → OKLab (смена осей на ходу).
    let order = [0_usize, 3, 2, 4, 1, 3, 0, 2, 4, 3];
    for (frame_index, &rule_index) in order.iter().enumerate() {
        let (name, rule) = &rules[rule_index];
        let frame = ScopeFrame {
            input: input.input(HorizontalMirror::Off),
            rule,
            settings: &settings,
            picture_size: picture_size(160, 96),
            frame_slot: frame_index % 2,
        };
        let (recorded, _) = run_scope_frame(&context, &mut scopes, &frame).unwrap();
        assert_eq!(
            recorded.accumulator,
            isp_vulkan::scopes::GpuScopeRule::accumulator(rule)
        );
        assert_eq!(
            scopes.read_back_counters_blocking().unwrap(),
            cpu_counters(&shown, &settings, rule as &dyn ScopeRule),
            "кадр {frame_index} ({name}): счётчики после смены прибора"
        );
        // Ни одного нового пайплайна ни по счётчику владельца, ни по живым объектам устройства.
        assert_eq!(scopes.accumulator_pipeline_count(), pipelines_at_creation);
        assert_eq!(
            context.live_objects().count(LiveObjectKind::Pipeline),
            live_pipelines,
            "кадр {frame_index}"
        );
    }
    let counters = scopes.pass_counters();
    assert_eq!(counters.recorded, order.len() as u64);
    assert_eq!(counters.accumulate_dispatches, order.len() as u64);
    assert_eq!(counters.render_dispatches, order.len() as u64);
    assert_eq!(counters.picture_recreations, 0, "картинка не пересоздавалась");
    assert_no_validation_errors(&context);
}

#[test]
fn picture_size_change_recreates_only_the_picture_and_retires_the_old_one_until_its_slot_fence() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = small_settings();
    let mut scopes = scopes_for(&context, &settings, 1);
    let input = upload_scene(&context, &bars75_scene(), StorageFormatPolicy::HalfFloatOnly, (0, 0));
    let shown = input.shown_codes(HorizontalMirror::Off);
    let rules = rule_cases(&settings);
    let (_, rule) = &rules[2];
    let live_pipelines = context.live_objects().count(LiveObjectKind::Pipeline);
    let frame_of = |size| ScopeFrame {
        input: input.input(HorizontalMirror::Off),
        rule,
        settings: &settings,
        picture_size: size,
        frame_slot: 0,
    };
    // `run_scope_frame` сразу подтверждает fence, поэтому ожидание старой картинки видно только через ручную запись.
    let (first, _) = run_scope_frame(&context, &mut scopes, &frame_of(picture_size(160, 96))).unwrap();
    let images_before = context.live_objects().count(LiveObjectKind::Image);
    assert_eq!(
        scopes.prepare_picture(picture_size(200, 120)).unwrap(),
        PictureChange::Replaced,
        "новая картинка готовится между кадрами"
    );
    let mut recorded = None;
    run_one_shot_blocking(&context, |command_buffer| {
        recorded = Some(scopes.record(command_buffer, &frame_of(picture_size(200, 120))));
        Ok(())
    })
    .unwrap();
    let second = recorded.unwrap().unwrap();
    assert_ne!(second.picture_generation, first.picture_generation, "картинка новая");
    assert_eq!((second.size.width(), second.size.height()), (200, 120));
    assert_eq!(scopes.pass_counters().picture_recreations, 1);
    assert_eq!(scopes.retired_pictures(), 1, "старая картинка ждёт fence слота");
    assert_eq!(
        context.live_objects().count(LiveObjectKind::Image),
        images_before + 1,
        "до fence живы обе картинки"
    );
    scopes.release_slot_after_fence(0).unwrap();
    assert_eq!(scopes.retired_pictures(), 0, "после fence старая картинка уничтожена");
    assert_eq!(
        context.live_objects().count(LiveObjectKind::Image),
        images_before,
        "память картинки возвращена"
    );
    // Пайплайны те же, счётчики верны, новая картинка прочитывается нужного размера.
    assert_eq!(context.live_objects().count(LiveObjectKind::Pipeline), live_pipelines);
    assert_eq!(
        scopes.read_back_counters_blocking().unwrap(),
        cpu_counters(&shown, &settings, rule as &dyn ScopeRule)
    );
    let picture = scopes.read_back_picture_blocking().unwrap();
    assert_eq!((picture.size.width(), picture.size.height()), (200, 120));
    assert_eq!(picture.texels.len(), 200 * 120);
    // Тот же размер подряд — без пересоздания.
    run_scope_frame(&context, &mut scopes, &frame_of(picture_size(200, 120))).unwrap();
    assert_eq!(scopes.pass_counters().picture_recreations, 1);
    assert_no_validation_errors(&context);
}

#[test]
fn scopes_never_modify_the_input_image() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = small_settings();
    let mut scopes = scopes_for(&context, &settings, 1);
    for policy in [
        StorageFormatPolicy::PreferTenBitUnorm,
        StorageFormatPolicy::HalfFloatOnly,
    ] {
        let input = upload_scene(&context, &random_scene(99), policy, (6, 4));
        let before = input.read_all();
        for (_, rule) in rule_cases(&settings) {
            for mirror in [HorizontalMirror::Off, HorizontalMirror::On] {
                let frame = ScopeFrame {
                    input: input.input(mirror),
                    rule: &rule,
                    settings: &settings,
                    picture_size: picture_size(160, 96),
                    frame_slot: 0,
                };
                run_scope_frame(&context, &mut scopes, &frame).unwrap();
            }
        }
        assert_eq!(input.read_all(), before, "{policy:?}: вход после приборов изменился");
    }
    assert_no_validation_errors(&context);
}

/// Зеркало по ФОРМУЛЕ: левая половина белая, правая чёрная. Без зеркала белое — в левых столбцах waveform,
/// с зеркалом — в правых (показанная картинка зеркальна).
#[test]
fn mirror_swaps_the_shown_columns_on_waveform() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = ScopeModelSettings {
        sample_stride: SampleStride::EVERY_PIXEL,
        ..small_settings()
    };
    let mut scopes = scopes_for(&context, &settings, 1);
    let scene = Scene {
        name: "half-white",
        width: 64,
        height: 4,
        codes: (0..64 * 4)
            .map(|index| if index % 64 < 32 { [1023; 3] } else { [0; 3] })
            .collect(),
    };
    let input = upload_scene(&context, &scene, StorageFormatPolicy::HalfFloatOnly, (0, 0));
    let rules = rule_cases(&settings);
    let (_, waveform) = &rules[0];
    let top_row = 0;
    let bottom_row = settings.trace_levels - 1;
    // Столбец сетки отсчёта x: `x · columns / width` (ширина 64). Крайние показанные столбцы: x = 0 и x = 63.
    let first_column = 0;
    let last_column = 63 * settings.trace_columns / 64;
    for (mirror, white_column, black_column) in [
        (HorizontalMirror::Off, first_column, last_column),
        (HorizontalMirror::On, last_column, first_column),
    ] {
        let frame = ScopeFrame {
            input: input.input(mirror),
            rule: waveform,
            settings: &settings,
            picture_size: picture_size(160, 96),
            frame_slot: 0,
        };
        run_scope_frame(&context, &mut scopes, &frame).unwrap();
        let counters = scopes.read_back_counters_blocking().unwrap();
        assert!(
            counters.count(0, top_row, white_column).unwrap() > 0,
            "{mirror:?}: белое сверху в своём столбце"
        );
        assert!(
            counters.count(0, bottom_row, black_column).unwrap() > 0,
            "{mirror:?}: чёрное снизу"
        );
        assert_eq!(
            counters.count(0, top_row, black_column),
            Some(0),
            "{mirror:?}: белого нет на чёрной стороне"
        );
        // Три плоскости (R', G', B') на каждый отсчёт: серая картинка попадает в каждую.
        assert_eq!(counters.total(), 64 * 4 * 3, "{mirror:?}: все отсчёты учтены");
    }
    assert_no_validation_errors(&context);
}

/// Каждый отказ — ДО первой команды: буфер счётчиков и картинка остаются от прошлого кадра, диагностика не растёт,
/// а следующий верный кадр записывается и считается правильно.
#[test]
fn rejected_frames_record_no_commands_and_change_nothing() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = small_settings();
    // Буфер счётчиков рассчитан на маленькие сетки: прибор с большой сеткой не помещается.
    let tiny_sizing = ScopeModelSettings {
        trace_columns: 16,
        trace_levels: 16,
        vectorscope_size: 32,
        ..small_settings()
    };
    let mut scopes = scopes_for(&context, &tiny_sizing, 2);
    let small_rules = rule_cases(&tiny_sizing);
    let big_rules = rule_cases(&settings);
    let input = upload_scene(&context, &narrow_scene(), StorageFormatPolicy::HalfFloatOnly, (2, 2));
    let shown = input.shown_codes(HorizontalMirror::Off);
    let (_, good_rule) = &small_rules[0];
    let good = ScopeFrame {
        input: input.input(HorizontalMirror::Off),
        rule: good_rule,
        settings: &tiny_sizing,
        picture_size: picture_size(64, 48),
        frame_slot: 0,
    };
    run_scope_frame(&context, &mut scopes, &good).unwrap();
    let good_counters = scopes.read_back_counters_blocking().unwrap();
    let counters_before = scopes.pass_counters();

    let try_frame = |scopes: &mut ColorScopes, frame: &ScopeFrame<'_>| -> Result<(), ScopeError> {
        // Отказ проверки записи не зависит от подготовки картинки: готовим её заранее, как владелец.
        scopes.prepare_picture(frame.picture_size)?;
        let mut outcome = None;
        run_one_shot_blocking(&context, |command_buffer| {
            outcome = Some(scopes.record(command_buffer, frame).map(|_| ()));
            Ok(())
        })
        .unwrap();
        outcome.unwrap()
    };

    let bad_slot = ScopeFrame { frame_slot: 5, ..good };
    assert_eq!(
        try_frame(&mut scopes, &bad_slot),
        Err(ScopeError::FrameSlotOutOfRange { slot: 5, slots: 2 })
    );

    let invalid_settings = ScopeModelSettings {
        trace_columns: 0,
        ..tiny_sizing.clone()
    };
    let bad_settings = ScopeFrame {
        settings: &invalid_settings,
        ..good
    };
    assert!(matches!(
        try_frame(&mut scopes, &bad_settings),
        Err(ScopeError::Settings(_))
    ));

    let mut wrong_format = input.input(HorizontalMirror::Off);
    wrong_format.view.format = vk::Format::R8G8B8A8_UNORM;
    let bad_format = ScopeFrame {
        input: wrong_format,
        ..good
    };
    assert_eq!(
        try_frame(&mut scopes, &bad_format),
        Err(ScopeError::UnsupportedInputFormat {
            format: vk::Format::R8G8B8A8_UNORM
        })
    );

    let too_wide = ScopeInput {
        view: input.image.sampled_view(),
        window: SourceWindow::of_content(vk::Extent2D { width: 500, height: 4 }),
    };
    let bad_window = ScopeFrame {
        input: too_wide,
        ..good
    };
    assert!(matches!(
        try_frame(&mut scopes, &bad_window),
        Err(ScopeError::InputWindow(GpuError::InvalidArgument { .. }))
    ));

    let (_, big_rule) = &big_rules[0];
    let big_grid = ScopeFrame {
        rule: big_rule,
        settings: &settings,
        ..good
    };
    assert!(matches!(
        try_frame(&mut scopes, &big_grid),
        Err(ScopeError::GridExceedsCounterCapacity { capacity: 1024, .. })
    ));

    // Ничего не записано и не изменено: счётчики GPU и диагностика — прежние.
    assert_eq!(scopes.pass_counters(), counters_before);
    assert_eq!(
        scopes.read_back_counters_blocking().unwrap(),
        good_counters,
        "буфер счётчиков не тронут"
    );

    // Верный кадр после отказов работает (другое правило, другой слот).
    let (_, ycbcr) = &small_rules[2];
    let next = ScopeFrame {
        rule: ycbcr,
        frame_slot: 1,
        ..good
    };
    run_scope_frame(&context, &mut scopes, &next).unwrap();
    assert_eq!(
        scopes.read_back_counters_blocking().unwrap(),
        cpu_counters(&shown, &tiny_sizing, ycbcr as &dyn ScopeRule)
    );
    let after = counters_delta(counters_before, scopes.pass_counters());
    assert_eq!((after.recorded, after.render_dispatches), (1, 1));
    assert_no_validation_errors(&context);
}

/// Набор дескрипторов слота, привязанный в командный буфер, нельзя менять до подтверждения fence: другой вход в
/// тот же слот без `release_slot_after_fence` — ошибка, а не молчаливая порча; после подтверждения — работает.
#[test]
fn a_slot_must_be_released_after_its_fence_before_binding_another_input() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = small_settings();
    let mut scopes = scopes_for(&context, &settings, 1);
    let rules = rule_cases(&settings);
    let (_, rule) = &rules[0];
    let first = upload_scene(&context, &bars75_scene(), StorageFormatPolicy::HalfFloatOnly, (0, 0));
    let second = upload_scene(&context, &random_scene(5), StorageFormatPolicy::HalfFloatOnly, (0, 0));
    let frame_of = |input: &scope_support::InputImage| ScopeFrame {
        input: input.input(HorizontalMirror::Off),
        rule,
        settings: &settings,
        picture_size: picture_size(160, 96),
        frame_slot: 0,
    };
    scopes.prepare_picture(picture_size(160, 96)).unwrap();
    let mut outcome = None;
    run_one_shot_blocking(&context, |command_buffer| {
        scopes.record(command_buffer, &frame_of(&first)).unwrap();
        outcome = Some(scopes.record(command_buffer, &frame_of(&second)));
        Ok(())
    })
    .unwrap();
    assert!(
        matches!(outcome.unwrap(), Err(ScopeError::Gpu(GpuError::InvalidState { .. }))),
        "второй вход в тот же слот без fence"
    );
    scopes.release_slot_after_fence(0).unwrap();
    run_scope_frame(&context, &mut scopes, &frame_of(&second)).unwrap();
    let shown = second.shown_codes(HorizontalMirror::Off);
    assert_eq!(
        scopes.read_back_counters_blocking().unwrap(),
        cpu_counters(&shown, &settings, rule as &dyn ScopeRule)
    );
    assert_no_validation_errors(&context);
}

/// Все сцены на одной картинке разных размеров окна — окно, не равное ёмкости, не читает «хвост».
#[test]
fn window_smaller_than_capacity_ignores_the_poisoned_tail() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = ScopeModelSettings {
        sample_stride: SampleStride::EVERY_PIXEL,
        ..small_settings()
    };
    let mut scopes = scopes_for(&context, &settings, 1);
    let rules = rule_cases(&settings);
    let (_, parade) = &rules[1];
    for scene in all_scenes() {
        let input = upload_scene(&context, &scene, StorageFormatPolicy::PreferTenBitUnorm, (9, 7));
        let frame = ScopeFrame {
            input: input.input(HorizontalMirror::Off),
            rule: parade,
            settings: &settings,
            picture_size: picture_size(96, 64),
            frame_slot: 0,
        };
        run_scope_frame(&context, &mut scopes, &frame).unwrap();
        let counters = scopes.read_back_counters_blocking().unwrap();
        // Каждый пиксель даёт ровно по одному попаданию на плоскость: сумма = 3 · пикселей СОДЕРЖИМОГО.
        assert_eq!(
            counters.total(),
            3 * u64::from(scene.width) * u64::from(scene.height),
            "{}",
            scene.name
        );
    }
    assert_no_validation_errors(&context);
}
