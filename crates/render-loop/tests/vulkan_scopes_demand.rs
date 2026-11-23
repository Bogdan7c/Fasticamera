//! Когда цветовые приборы считаются (ревью сессии 14h, решения 189, 190) через НАСТОЯЩИЙ бэкенд показа:
//! * приборы считаются ТОЛЬКО пока установлен источник UI: `Show` без источника отвергается, ничего не создаёт и
//!   проходов не пишет; `Install` → `Show` → проходы; снятие источника отзывает запрос;
//! * объекты GPU приборов готовятся МЕЖДУ кадрами: запись кадра не создаёт ни одного объекта GPU, а первый кадр после
//!   подготовки уже пишет проход;
//! * уход источника UI отзывает запрос в `ScopePass` СРАЗУ (без кадров), и пересоздание под новый формат цели после
//!   этого не строит приборы по устаревшему запросу;
//! * диагностика показывает факт устройства об округлении и денормалях f32.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod scope_rig;
mod support;
mod viewfinder_rig;
mod vulkan_support;

use render_loop::backend::DisplayBackend;
use render_loop::ui_overlay::UiOverlayCommand;
use render_loop::viewfinder::{ScopeResources, ViewfinderCommandOutcome};
use scope_contract::{ScopeIntent, ScopeKind, VectorscopeAxes};
use scope_rig::{
    FRAMES_TO_FLUSH_SLOTS, IdleSource, WARM_UP_FRAMES, apply_scopes, flush_frames, install, object_differences,
    pipeline_objects, request, scopes_of, start, start_without_ui, waveform,
};
use viewfinder_rig::{neutral_gray, reddish};

#[test]
fn show_without_a_ui_source_is_rejected_and_costs_nothing_then_install_and_show_computes_and_removal_withdraws() {
    let scenes = vec![neutral_gray(); WARM_UP_FRAMES + 4 + 2 * FRAMES_TO_FLUSH_SLOTS];
    let Some(mut bench) = start_without_ui(&scenes) else {
        return;
    };
    flush_frames(&mut bench, WARM_UP_FRAMES);
    let baseline = bench.backend.context().live_objects();

    // Без источника UI показывать картинку прибора некому: команда отвергнута и ничего не запомнено.
    let outcome = apply_scopes(&mut bench, ScopeIntent::Show(waveform()));
    let ViewfinderCommandOutcome::Rejected { reason } = outcome else {
        panic!("ожидался отказ без источника UI, а не {outcome:?}");
    };
    assert!(reason.contains("источник"), "причина названа: {reason}");
    flush_frames(&mut bench, 3);
    let rejected = scopes_of(&mut bench);
    assert_eq!(rejected.requested, None, "запрос не запомнен");
    assert_eq!(rejected.residency.resources, ScopeResources::Absent);
    assert_eq!(
        (
            rejected.counters.passes_recorded,
            rejected.counters.resources_created,
            rejected.counters.record_failures
        ),
        (0, 0, 0),
        "ни проходов, ни объектов, ни отказов"
    );
    assert_eq!(
        object_differences(&baseline, &bench.backend.context().live_objects()),
        Vec::new(),
        "отвергнутая команда не создала ни одного объекта GPU"
    );

    // Источник установлен — тот же Show принят, приборы считают и есть что рисовать.
    install(&mut bench, Box::new(IdleSource));
    assert_eq!(
        apply_scopes(&mut bench, ScopeIntent::Show(waveform())),
        ViewfinderCommandOutcome::Applied
    );
    flush_frames(&mut bench, 2);
    let computing = scopes_of(&mut bench);
    assert_eq!(computing.requested, Some(waveform()));
    assert_eq!(computing.residency.resources, ScopeResources::Live);
    assert_eq!(computing.counters.passes_recorded, 2, "по проходу на новый кадр");

    // Снятие источника отзывает запрос: проходов больше нет, объекты освобождены.
    assert_eq!(
        bench.backend.apply_ui_overlay_command(UiOverlayCommand::Remove),
        render_loop::ui_overlay::UiOverlayOutcome::Applied
    );
    flush_frames(&mut bench, FRAMES_TO_FLUSH_SLOTS);
    let withdrawn = scopes_of(&mut bench);
    assert_eq!((withdrawn.requested, withdrawn.withdrawals), (None, 1));
    assert_eq!(
        withdrawn.counters.passes_recorded, 2,
        "после снятия источника проходов нет"
    );
    assert_eq!(withdrawn.residency.resources, ScopeResources::Absent);
    assert_eq!(
        withdrawn.counters.resources_created, withdrawn.counters.resources_released,
        "объекты приборов уничтожены"
    );
    // Show снова отвергается, пока источника нет.
    assert!(matches!(
        apply_scopes(&mut bench, ScopeIntent::Show(waveform())),
        ViewfinderCommandOutcome::Rejected { .. }
    ));
    bench.finish();
}

#[test]
fn recording_a_frame_never_creates_gpu_objects_and_the_first_frame_after_the_prepare_step_records_a_pass() {
    let scenes = vec![reddish(); WARM_UP_FRAMES + 4];
    let Some(mut bench) = start(&scenes) else { return };
    // Прогрев с источником UI: ISP, компоновщик, UI и статистика уже построили свои ленивые объекты.
    flush_frames(&mut bench, WARM_UP_FRAMES);

    // Команда Show — шаг подготовки: набор приборов и картинка создаются здесь, между кадрами.
    assert_eq!(
        apply_scopes(&mut bench, ScopeIntent::Show(waveform())),
        ViewfinderCommandOutcome::Applied
    );
    assert_eq!(
        scopes_of(&mut bench).residency.resources,
        ScopeResources::Live,
        "набор создан до первого кадра"
    );
    let before_frame = bench.backend.context().live_objects();
    bench.show_next_frame();
    let after_frame = bench.backend.context().live_objects();
    let recorded = scopes_of(&mut bench);
    assert_eq!(
        recorded.counters.passes_recorded, 1,
        "первый кадр после подготовки уже пишет проход"
    );
    assert_eq!(recorded.counters.frames_skipped_not_ready, 0);
    assert_eq!(
        object_differences(&before_frame, &after_frame),
        Vec::new(),
        "запись кадра с приборами не создала и не уничтожила ни одного объекта GPU"
    );

    // Смена размера картинки: новая картинка создаётся командой (между кадрами), а не в кадре.
    let resized = request(ScopeKind::Waveform, VectorscopeAxes::YCbCrBt709, 200, 120);
    assert_eq!(
        apply_scopes(&mut bench, ScopeIntent::Show(resized)),
        ViewfinderCommandOutcome::Applied
    );
    let before_resized_frame = bench.backend.context().live_objects();
    bench.show_next_frame();
    let after_resized_frame = bench.backend.context().live_objects();
    let resized_report = scopes_of(&mut bench);
    assert_eq!(resized_report.counters.passes_recorded, 2);
    assert_eq!(resized_report.counters.frames_skipped_not_ready, 0);
    assert_eq!(
        object_differences(&before_resized_frame, &after_resized_frame),
        Vec::new(),
        "и при смене размера запись кадра объектов не создаёт"
    );
    bench.finish();
}

#[test]
fn diagnostics_expose_the_device_float_controls_as_a_fact() {
    let scenes = vec![neutral_gray(); 2];
    let Some(mut bench) = start(&scenes) else { return };
    bench.show_next_frame();
    let reported = scopes_of(&mut bench).float32_controls;
    let device = bench.backend.context().capabilities().float32_controls;
    assert_eq!(
        reported, device,
        "диагностика приборов показывает возможности устройства"
    );
    // Побитовый паритет счётчиков опирается на округление к ближайшему чётному: устройство, где его нельзя даже
    // потребовать, требует отдельного внимания, а не молчаливого допущения.
    assert!(
        reported.rounding_mode_rte,
        "устройство не поддерживает shaderRoundingModeRTEFloat32: допущение паритета не проверить"
    );
    bench.finish();
}

/// Уход источника UI сразу отзывает запрос и в `ScopePass`: пауза (кадров нет) не держит набор «живым».
#[test]
fn removing_the_ui_source_retires_the_live_scope_set_at_once_even_when_no_frame_follows() {
    let scenes = vec![neutral_gray(); WARM_UP_FRAMES + 4 + FRAMES_TO_FLUSH_SLOTS];
    let Some(mut bench) = start(&scenes) else { return };
    flush_frames(&mut bench, WARM_UP_FRAMES);
    assert_eq!(
        apply_scopes(&mut bench, ScopeIntent::Show(waveform())),
        ViewfinderCommandOutcome::Applied
    );
    flush_frames(&mut bench, 2);
    let computing = scopes_of(&mut bench);
    assert_eq!(computing.residency.resources, ScopeResources::Live);
    assert!(computing.residency.accumulator_pipelines > 0, "набор приборов построен");

    // Источник снят, ни одного кадра после этого не рисуется.
    assert_eq!(
        bench.backend.apply_ui_overlay_command(UiOverlayCommand::Remove),
        render_loop::ui_overlay::UiOverlayOutcome::Applied
    );
    let paused = scopes_of(&mut bench);
    assert_eq!(paused.requested, None, "запрос отозван");
    assert!(
        matches!(
            paused.residency.resources,
            ScopeResources::Retiring | ScopeResources::Absent
        ),
        "набор выведен сразу, а не жив до следующего кадра: {:?}",
        paused.residency
    );
    assert_eq!(paused.residency.accumulator_pipelines, 0, "действующего набора нет");
    // Кадры освобождают слоты: набор доживает ровно до конца кадров в полёте и уничтожается.
    flush_frames(&mut bench, FRAMES_TO_FLUSH_SLOTS);
    let released = scopes_of(&mut bench);
    assert_eq!(released.residency.resources, ScopeResources::Absent);
    assert_eq!(
        released.counters.resources_created,
        released.counters.resources_released
    );
    assert_eq!(released.counters.passes_recorded, computing.counters.passes_recorded);
    bench.finish();
}

/// Пересоздание под новый формат цели берёт запрос у владельца настроек, а не из закэшированного: после ухода
/// источника UI оно не строит набор приборов и картинку.
#[test]
fn a_format_rebuild_right_after_the_ui_source_left_builds_no_scope_objects() {
    let scenes = vec![neutral_gray(); WARM_UP_FRAMES + 4 + FRAMES_TO_FLUSH_SLOTS];
    let Some(mut bench) = start(&scenes) else { return };
    flush_frames(&mut bench, WARM_UP_FRAMES);
    // Объекты без приборов: исходный уровень (ISP, компоновщик, UI уже построены).
    let pipelines_without_scopes = pipeline_objects(&bench);
    assert_eq!(
        apply_scopes(&mut bench, ScopeIntent::Show(waveform())),
        ViewfinderCommandOutcome::Applied
    );
    flush_frames(&mut bench, 2);
    assert!(
        pipeline_objects(&bench) > pipelines_without_scopes,
        "приборы построили свои пайплайны"
    );
    bench.backend.apply_ui_overlay_command(UiOverlayCommand::Remove);
    let before_rebuild = scopes_of(&mut bench);

    // Пересоздание цели ДО любого кадра после ухода источника: кадр ещё не успел бы обновить запрос сам.
    bench
        .backend
        .rebuild_for_format_change_for_tests()
        .expect("пересоздание под формат");
    let after_rebuild = scopes_of(&mut bench);
    assert_eq!(
        after_rebuild.counters.resources_created, before_rebuild.counters.resources_created,
        "набор приборов не создавался по устаревшему запросу"
    );
    assert_eq!(after_rebuild.residency.resources, ScopeResources::Absent);
    assert_eq!(after_rebuild.requested, None);
    assert!(
        pipeline_objects(&bench) <= pipelines_without_scopes,
        "пайплайнов приборов нет: {} против исходных {pipelines_without_scopes}",
        pipeline_objects(&bench)
    );

    // Источник вернулся и снова просит прибор: набор поднимается заново (запрос не потерян, а свежий).
    install(&mut bench, Box::new(IdleSource));
    assert_eq!(
        apply_scopes(&mut bench, ScopeIntent::Show(waveform())),
        ViewfinderCommandOutcome::Applied
    );
    bench
        .backend
        .rebuild_for_format_change_for_tests()
        .expect("пересоздание под формат");
    assert_eq!(scopes_of(&mut bench).residency.resources, ScopeResources::Live);
    bench.finish();
}
