//! Картинка прибора: подготовка ЗАРАНЕЕ (между кадрами) и порядок «все проверки до смены состояния»
//! (ревью сессии 14h, замечания 2 и 5).
//!
//! Доказывается:
//! * `prepare_picture` создаёт картинку без записи кадра, а последующая запись объектов GPU уже не создаёт;
//! * замена картинки считается и уходит в отложенное уничтожение в момент замены;
//! * отказ ПОЗДНЕЙ проверки записи (привязка шейдера показа) оставляет действующую картинку на месте,
//!   счётчики не меняются, новая картинка не утекает и нигде не остаётся привязанной.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod scope_support;

use gpu_vulkan::error::GpuError;
use gpu_vulkan::kit_test_support::run_one_shot_blocking;
use gpu_vulkan::live_objects::LiveObjectKind;
use gpu_vulkan::source_window::HorizontalMirror;
use gpu_vulkan::storage_image::StorageFormatPolicy;
use isp_vulkan::scopes::{ColorScopes, ColorScopesConfig, PictureChange, ScopeError, ScopeFrame};
use scope_model::ScopeRule;
use scope_support::{
    assert_no_validation_errors, bars75_scene, cpu_counters, gpu_context_or_skip, picture_size, rule_cases,
    run_scope_frame, small_settings, upload_scene,
};

fn image_count(context: &gpu_vulkan::context::GpuContext) -> usize {
    context.live_objects().count(LiveObjectKind::Image)
}

#[test]
fn a_prepared_picture_makes_the_first_record_create_no_gpu_objects_and_still_measure_correctly() {
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
    let input = upload_scene(&context, &bars75_scene(), StorageFormatPolicy::HalfFloatOnly, (0, 0));
    let shown = input.shown_codes(HorizontalMirror::Off);
    let rules = rule_cases(&settings);
    let (_, rule) = &rules[0];
    let size = picture_size(160, 96);
    let frame = ScopeFrame {
        input: input.input(HorizontalMirror::Off),
        rule,
        settings: &settings,
        picture_size: size,
        frame_slot: 0,
    };

    assert!(!scopes.is_picture_ready_for(size), "до подготовки картинки нет");
    let images_before = image_count(&context);
    assert_eq!(scopes.prepare_picture(size).unwrap(), PictureChange::Created);
    assert_eq!(image_count(&context), images_before + 1, "подготовка создала картинку");
    assert!(scopes.is_picture_ready_for(size));
    assert_eq!(
        scopes.prepare_picture(size).unwrap(),
        PictureChange::Unchanged,
        "повтор того же размера ничего не создаёт"
    );
    assert_eq!(image_count(&context), images_before + 1);

    run_scope_frame(&context, &mut scopes, &frame).unwrap();
    assert_eq!(
        image_count(&context),
        images_before + 1,
        "запись кадра с подготовленной картинкой объектов GPU не создаёт"
    );
    assert_eq!(scopes.pass_counters().recorded, 1);
    assert_eq!(
        scopes.pass_counters().picture_recreations,
        0,
        "первое создание — не замена"
    );
    assert_eq!(
        scopes.read_back_counters_blocking().unwrap(),
        cpu_counters(&shown, &settings, rule as &dyn ScopeRule),
        "подготовленная картинка измеряет так же, как созданная в записи"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn preparing_another_size_replaces_the_picture_counts_it_and_retires_the_old_one_until_a_later_record_completes() {
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
    let input = upload_scene(&context, &bars75_scene(), StorageFormatPolicy::HalfFloatOnly, (0, 0));
    let rules = rule_cases(&settings);
    let (_, rule) = &rules[0];
    let frame_of = |width, height| ScopeFrame {
        input: input.input(HorizontalMirror::Off),
        rule,
        settings: &settings,
        picture_size: picture_size(width, height),
        frame_slot: 0,
    };
    run_scope_frame(&context, &mut scopes, &frame_of(160, 96)).unwrap();
    let old_view = scopes.output_view().unwrap();
    let images_before = image_count(&context);

    assert_eq!(
        scopes.prepare_picture(picture_size(200, 120)).unwrap(),
        PictureChange::Replaced
    );
    assert_eq!(scopes.pass_counters().picture_recreations, 1);
    assert_eq!(scopes.retired_pictures(), 1, "старая картинка ждёт завершения записи");
    assert_eq!(image_count(&context), images_before + 1, "до завершения живы обе");
    assert!(scopes.is_picture_ready_for(picture_size(200, 120)));
    assert!(!scopes.is_picture_ready_for(picture_size(160, 96)));
    assert_ne!(
        scopes.output_view().unwrap().view,
        old_view.view,
        "действующая картинка новая"
    );

    // Следующая запись на новом размере ничего не создаёт и не считает замену повторно; после её fence старая
    // картинка уничтожается.
    run_scope_frame(&context, &mut scopes, &frame_of(200, 120)).unwrap();
    assert_eq!(scopes.pass_counters().picture_recreations, 1);
    assert_eq!(scopes.retired_pictures(), 0);
    assert_eq!(image_count(&context), images_before, "старая картинка уничтожена");
    assert_no_validation_errors(&context);
}

/// Отказ поздней проверки записи: два `record` в один слот без fence — второй отказывает на привязке шейдера
/// показа, КОГДА новая картинка уже создана проверками. Она не должна ни стать действующей, ни остаться привязанной.
#[test]
fn a_late_check_failure_keeps_the_current_picture_and_changes_no_counter() {
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
    let input = upload_scene(&context, &bars75_scene(), StorageFormatPolicy::HalfFloatOnly, (0, 0));
    let shown = input.shown_codes(HorizontalMirror::Off);
    let rules = rule_cases(&settings);
    let (_, rule) = &rules[0];
    let frame_of = |width, height| ScopeFrame {
        input: input.input(HorizontalMirror::Off),
        rule,
        settings: &settings,
        picture_size: picture_size(width, height),
        frame_slot: 0,
    };
    let images_before = image_count(&context);
    let mut view_before_failure = None;
    let mut counters_before_failure = None;
    let mut outcome = None;
    run_one_shot_blocking(&context, |command_buffer| {
        scopes.record(command_buffer, &frame_of(160, 96)).unwrap();
        view_before_failure = scopes.output_view();
        counters_before_failure = Some(scopes.pass_counters());
        // Другой размер в тот же слот без fence: набор шейдера показа уже привязан в буфер — привязка новой картинки
        // отказывает уже после создания новой картинки.
        outcome = Some(scopes.record(command_buffer, &frame_of(200, 120)));
        Ok(())
    })
    .unwrap();
    assert!(
        matches!(outcome.unwrap(), Err(ScopeError::Gpu(GpuError::InvalidState { .. }))),
        "поздняя проверка отказала"
    );
    assert_eq!(
        scopes.output_view(),
        view_before_failure,
        "действующая картинка прежняя, а не заменённая"
    );
    assert_eq!(
        scopes.pass_counters(),
        counters_before_failure.unwrap(),
        "счётчики не изменились"
    );
    assert_eq!(
        scopes.retired_pictures(),
        0,
        "прежняя картинка не выведена из обращения"
    );
    assert_eq!(
        image_count(&context),
        images_before + 1,
        "созданная проверками новая картинка уничтожена, не утекла"
    );

    // После fence слот снова годен: тот же кадр нового размера проходит, замена считается ровно один раз, а
    // созданная заново картинка не наследует привязку отказавшей (validation молчит, счётчики верны).
    scopes.release_slot_after_fence(0).unwrap();
    run_scope_frame(&context, &mut scopes, &frame_of(200, 120)).unwrap();
    assert_eq!(scopes.pass_counters().picture_recreations, 1);
    assert_eq!(
        scopes.read_back_counters_blocking().unwrap(),
        cpu_counters(&shown, &settings, rule as &dyn ScopeRule)
    );
    assert_no_validation_errors(&context);
}
