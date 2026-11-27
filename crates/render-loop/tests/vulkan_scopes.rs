//! Цветовые приборы поверх видоискателя (сессия 14h, решения 189, 190) через НАСТОЯЩИЙ бэкенд показа: RADV,
//! headless-показ, полная validation, подставная камера (стенд `viewfinder_rig`).
//!
//! Доказывается:
//! * прибор считает ровно тот кадр, что на экране, и на КАЖДОМ новом кадре: счётчики GPU после кадра равны эталону
//!   `scope-model` по прочитанной картинке видоискателя ЭТОГО кадра (кадры разные, поэтому «мерил другой кадр»
//!   провалило бы тест); перекомпоновка проходов не пишет;
//! * инвариант 1: пиксели видоискателя и статистика AWB с приборами и без них побитно совпадают;
//! * инвариант 2: после «скрыть» проходов больше нет, объекты GPU приборов уничтожены (число пайплайнов вернулось к
//!   исходному), validation молчит;
//! * смена типа, осей и размера картинки на ходу не создаёт новых пайплайнов и не пересоздаёт набор;
//! * уход источника UI (снятие, замена, паника) отзывает запрос и освобождает объекты;
//! * картинка прибора доходит до рисования UI: элемент «живая картинка» рисуется, когда прибор показан, и
//!   пропускается без сбоя, когда скрыт.

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

use gpu_vulkan::live_objects::LiveObjectKind;
use render_loop::backend::DisplayBackend;
use render_loop::ui_overlay::{UiOverlayCommand, UiOverlayOutcome};
use render_loop::viewfinder::{ScopeResources, ViewfinderCommandOutcome};
use scope_contract::{ScopeIntent, ScopeKind, ScopeRequest, ScopeScan, VectorscopeAxes};
use scope_model::{HorizontalOrder, ResolvedScope, ScopeCounters, ScopeRule, ShownCodes, accumulate};
use scope_rig::{
    FRAMES_TO_FLUSH_SLOTS, IdleSource, WARM_UP_FRAMES, apply_scopes, config, flush_frames, install, pipeline_objects,
    request, request_with_scan, scopes_of, settle, small_model, start, start_with_auto_exposure, start_without_ui,
    waveform,
};
use ui_draw::color::Opacity;
use ui_draw::draw_list::{DrawItem, DrawList, ImageClip, ImageFit, ImageItem, ImageSource, LiveImage};
use ui_draw::frame_source::{UiFrameError, UiFrameRequest, UiFrameSource};
use ui_draw::geometry::PxSize;
use ui_draw::light::LightDirection;
use ui_draw::transform::Affine2;
use viewfinder_rig::{CellValues, ViewfinderBench, neutral_gray, reddish};

/// Бровь синеватая: третий сюжет, отличный от серого и красноватого.
fn bluish() -> CellValues {
    CellValues {
        red: 220,
        green: 300,
        blue: 620,
    }
}

/// Счётчики, посчитанные GPU в последнем записанном проходе.
fn gpu_counters(bench: &ViewfinderBench) -> ScopeCounters {
    settle(bench);
    bench
        .backend
        .read_scope_counters()
        .expect("чтение счётчиков приборов")
        .expect("набор приборов есть и проход записан")
}

/// Эталон: счётчики `scope-model` по картинке видоискателя, прочитанной с GPU (то, что показано на экране).
fn cpu_counters(bench: &ViewfinderBench, scope: &ScopeRequest) -> ScopeCounters {
    settle(bench);
    let image = bench
        .backend
        .read_viewfinder_image()
        .expect("чтение изображения видоискателя")
        .expect("изображение видоискателя есть");
    let shown = ShownCodes::from_display_rgba(image.width, image.height, &image.pixels, HorizontalOrder::AsStored)
        .expect("коды");
    let model = small_model();
    let rule = ResolvedScope::resolve(scope, &model, isp_model::image::DisplayGamut::Srgb).expect("правило");
    accumulate(
        &shown,
        rule.planned_samples(shown.width(), shown.height(), &model).steps(),
        &rule as &dyn ScopeRule,
    )
    .expect("эталон")
}

#[test]
fn scope_counters_equal_the_cpu_reference_of_each_shown_frame_and_one_pass_is_recorded_per_new_frame() {
    let scenes = [neutral_gray(), reddish(), bluish(), reddish()];
    let Some(mut bench) = start(&scenes) else { return };
    let scope = waveform();
    assert_eq!(
        apply_scopes(&mut bench, ScopeIntent::Show(scope)),
        ViewfinderCommandOutcome::Applied
    );
    let mut previous: Option<ScopeCounters> = None;
    for shown in 1..=scenes.len() as u64 {
        bench.show_next_frame();
        let diagnostics = scopes_of(&mut bench);
        assert_eq!(
            diagnostics.counters.passes_recorded, shown,
            "ровно один проход на новый кадр (перекомпоновка проходов не пишет)"
        );
        let gpu = gpu_counters(&bench);
        let cpu = cpu_counters(&bench, &scope);
        assert_eq!(
            gpu, cpu,
            "кадр {shown}: счётчики GPU разошлись с эталоном по показанной картинке"
        );
        assert!(cpu.total() > 0, "кадр {shown}: прибор что-то сосчитал");
        if let Some(previous) = &previous {
            assert_ne!(previous, &cpu, "кадр {shown}: сюжет сменился, значит и счётчики другие");
        }
        previous = Some(cpu);
    }
    let diagnostics = scopes_of(&mut bench);
    assert_eq!(diagnostics.requested, Some(scope));
    assert_eq!(diagnostics.residency.resources, ScopeResources::Live);
    assert_eq!(
        (
            diagnostics.counters.resources_created,
            diagnostics.counters.record_failures
        ),
        (1, 0)
    );
    bench.finish();
}

#[test]
fn scopes_do_not_change_the_viewfinder_picture_or_the_awb_statistics() {
    let scenes = [neutral_gray(), reddish(), bluish()];
    let mut runs: Vec<Vec<(Vec<[f32; 4]>, String)>> = Vec::new();
    for with_scopes in [false, true] {
        // Статистику кадра считает только наш AE: без него публиковать нечего и сравнивать нечего.
        let Some(mut bench) = start_with_auto_exposure(&scenes) else {
            return;
        };
        if with_scopes {
            apply_scopes(&mut bench, ScopeIntent::Show(waveform()));
        }
        let mut observed = Vec::new();
        for _ in &scenes {
            bench.show_next_frame();
            settle(&bench);
            let pixels = bench
                .backend
                .read_viewfinder_image()
                .expect("чтение")
                .expect("изображение")
                .pixels;
            let statistics = bench
                .backend
                .awb_statistics()
                .latest()
                .map(|published| format!("{:?}", published.statistics))
                .unwrap_or_default();
            observed.push((pixels, statistics));
        }
        if with_scopes {
            assert_eq!(
                scopes_of(&mut bench).counters.passes_recorded,
                scenes.len() as u64,
                "в прогоне с приборами они реально считались"
            );
        }
        bench.finish();
        runs.push(observed);
    }
    assert!(
        runs[0].iter().any(|(_, statistics)| !statistics.is_empty()),
        "статистика AWB публикуется"
    );
    assert_eq!(
        runs[0], runs[1],
        "пиксели и статистика AWB не зависят от приборов (инвариант 1)"
    );
}

#[test]
fn hiding_stops_the_passes_and_releases_every_scope_object_without_leaks() {
    let scenes = vec![neutral_gray(); WARM_UP_FRAMES + 3 + FRAMES_TO_FLUSH_SLOTS];
    let Some(mut bench) = start(&scenes) else { return };
    // Прогрев без приборов: ISP, компоновщик, статистика AWB и прочее ленивое уже построены — это исходная точка.
    flush_frames(&mut bench, WARM_UP_FRAMES);
    let baseline = pipeline_objects(&bench);
    assert_eq!(scopes_of(&mut bench).residency.resources, ScopeResources::Absent);
    let query_pools_before = bench.backend.context().live_objects().count(LiveObjectKind::QueryPool);

    apply_scopes(&mut bench, ScopeIntent::Show(waveform()));
    flush_frames(&mut bench, 2);
    let shown = scopes_of(&mut bench);
    assert_eq!(shown.residency.resources, ScopeResources::Live);
    assert_eq!(shown.counters.passes_recorded, 2);
    assert!(pipeline_objects(&bench) > baseline, "приборы построили пайплайны");
    assert!(
        bench.backend.context().live_objects().count(LiveObjectKind::QueryPool) > query_pools_before,
        "и свои метки времени"
    );

    assert_eq!(
        apply_scopes(&mut bench, ScopeIntent::Hide),
        ViewfinderCommandOutcome::Applied
    );
    flush_frames(&mut bench, FRAMES_TO_FLUSH_SLOTS);
    let hidden = scopes_of(&mut bench);
    assert_eq!(hidden.counters.passes_recorded, 2, "после «скрыть» проходов нет");
    assert_eq!(hidden.residency.resources, ScopeResources::Absent);
    assert_eq!(
        (hidden.counters.resources_created, hidden.counters.resources_released),
        (1, 1)
    );
    assert_eq!(
        pipeline_objects(&bench),
        baseline,
        "объекты приборов уничтожены, ничего не утекло"
    );
    assert_eq!(
        bench.backend.context().live_objects().count(LiveObjectKind::QueryPool),
        query_pools_before
    );
    assert_eq!(
        apply_scopes(&mut bench, ScopeIntent::Hide),
        ViewfinderCommandOutcome::AlreadyInEffect
    );
    bench.finish();
}

#[test]
fn kind_axes_and_size_switch_on_the_fly_without_new_pipelines_and_with_correct_counters() {
    let scenes = vec![reddish(); WARM_UP_FRAMES + 6];
    let Some(mut bench) = start(&scenes) else { return };
    flush_frames(&mut bench, WARM_UP_FRAMES);
    let sequence = [
        request(ScopeKind::Waveform, VectorscopeAxes::YCbCrBt709, 160, 96),
        request(ScopeKind::RgbParade, VectorscopeAxes::YCbCrBt709, 160, 96),
        request(ScopeKind::Vectorscope, VectorscopeAxes::YCbCrBt709, 160, 96),
        request(ScopeKind::Vectorscope, VectorscopeAxes::OklabAb, 160, 96),
        // Смена размера картинки (поворот раздела) — пересоздаётся только картинка внутри набора.
        request(ScopeKind::Vectorscope, VectorscopeAxes::OklabAb, 100, 140),
        request(ScopeKind::Waveform, VectorscopeAxes::OklabAb, 100, 140),
    ];
    let mut pipelines_after_first = None;
    for (index, scope) in sequence.iter().enumerate() {
        assert_ne!(
            apply_scopes(&mut bench, ScopeIntent::Show(*scope)),
            ViewfinderCommandOutcome::AlreadyInEffect,
            "{scope:?}"
        );
        bench.show_next_frame();
        assert_eq!(
            gpu_counters(&bench),
            cpu_counters(&bench, scope),
            "шаг {index}: {scope:?}"
        );
        let picture = bench
            .backend
            .read_scope_picture()
            .expect("чтение картинки прибора")
            .expect("картинка есть");
        assert_eq!(
            (picture.size.width(), picture.size.height()),
            (scope.image_size.width(), scope.image_size.height()),
            "шаг {index}: размер картинки прибора"
        );
        let pipelines = pipeline_objects(&bench);
        assert_eq!(
            *pipelines_after_first.get_or_insert(pipelines),
            pipelines,
            "шаг {index}: переключение не строит пайплайнов"
        );
    }
    let diagnostics = scopes_of(&mut bench);
    assert_eq!(
        diagnostics.counters.resources_created, 1,
        "набор один на все переключения"
    );
    assert_eq!(diagnostics.counters.passes_recorded, sequence.len() as u64);
    assert!(
        diagnostics.residency.accumulator_pipelines >= 2,
        "пайплайны накопления построены при создании"
    );
    bench.finish();
}

#[test]
fn scan_switch_on_the_fly_changes_only_parameters_and_counters_follow_the_cpu_reference() {
    let scenes = vec![reddish(); WARM_UP_FRAMES + 10];
    let Some(mut bench) = start(&scenes) else { return };
    flush_frames(&mut bench, WARM_UP_FRAMES);
    let mut pipelines_after_first = None;
    let mut steps = 0_u64;
    for kind in [ScopeKind::Waveform, ScopeKind::RgbParade] {
        for scan in ScopeScan::ALL {
            let scope = request_with_scan(kind, VectorscopeAxes::YCbCrBt709, scan, 160, 96);
            apply_scopes(&mut bench, ScopeIntent::Show(scope));
            bench.show_next_frame();
            assert_eq!(gpu_counters(&bench), cpu_counters(&bench, &scope), "{kind:?} {scan:?}");
            let pipelines = pipeline_objects(&bench);
            assert_eq!(
                *pipelines_after_first.get_or_insert(pipelines),
                pipelines,
                "{kind:?} {scan:?}: смена положения не строит пайплайнов"
            );
            steps += 1;
        }
    }
    let diagnostics = scopes_of(&mut bench);
    assert_eq!(
        diagnostics.counters.resources_created, 1,
        "набор приборов один на все смены положения"
    );
    assert_eq!(diagnostics.counters.passes_recorded, steps);
    bench.finish();
}

/// Источник UI: рисует картинку цветового прибора по центру (как будет делать экран калибровки). Элемент рисования
/// имеет РОВНО размер запрошенной картинки: экран кладёт картинку 1:1 (решение 203), а рендер не отдаёт рисованию
/// картинку, чей размер не равен размеру элемента (`scope_fit.rs`; такой кадр считается `live_images_missing`).
struct ScopePictureSource {
    /// Размер картинки прибора, которую запрашивает тест (и в котором её рисует элемент).
    picture_size: PxSize,
}

impl ScopePictureSource {
    /// Источник, рисующий картинку в размере запроса `request`.
    fn drawing_at_size_of(request: &ScopeRequest) -> Self {
        let size = request.image_size;
        Self {
            picture_size: PxSize::new(size.width() as f32, size.height() as f32),
        }
    }
}

impl UiFrameSource for ScopePictureSource {
    fn name(&self) -> &'static str {
        "scope-picture-test"
    }

    fn produce_frame(&mut self, request: &UiFrameRequest) -> Result<DrawList, UiFrameError> {
        let mut list = DrawList::new(LightDirection::from_angle_degrees(45.0));
        list.items.push(DrawItem::Image(ImageItem {
            source: ImageSource::Live(LiveImage::ColorScope),
            transform: Affine2::translate(request.logical_size.width * 0.5, request.logical_size.height * 0.5),
            size_px: self.picture_size,
            fit: ImageFit::Stretch,
            clip: ImageClip::None,
            opacity: Opacity::OPAQUE,
        }));
        Ok(list)
    }
}

/// Источник UI, который падает на первом же кадре.
struct PanickingSource;

impl UiFrameSource for PanickingSource {
    fn name(&self) -> &'static str {
        "panicking-test"
    }

    fn produce_frame(&mut self, _request: &UiFrameRequest) -> Result<DrawList, UiFrameError> {
        panic!("сломанный экран приборов");
    }
}

#[test]
fn scope_picture_reaches_ui_drawing_and_a_hidden_scope_is_skipped_without_failure() {
    let scenes = vec![neutral_gray(); 12 + FRAMES_TO_FLUSH_SLOTS];
    let Some(mut bench) = start_without_ui(&scenes) else {
        return;
    };
    install(
        &mut bench,
        Box::new(ScopePictureSource::drawing_at_size_of(&waveform())),
    );
    // Прибор закрыт: элемент пропускается и считается, кадр нарисован.
    bench.show_next_frame();
    let hidden = bench.backend.diagnostics().viewfinder.expect("видоискатель").ui_overlay;
    assert!(hidden.frames_drawn > 0, "UI рисуется");
    assert_eq!(hidden.live_images_drawn, 0);
    assert_eq!(
        hidden.live_images_missing, hidden.frames_drawn,
        "по одному пропуску на кадр UI"
    );
    assert_eq!(
        (hidden.frames_failed, hidden.source_failures),
        (0, 0),
        "пропуск — не сбой"
    );

    apply_scopes(&mut bench, ScopeIntent::Show(waveform()));
    bench.show_next_frame();
    let before = bench.backend.diagnostics().viewfinder.expect("видоискатель").ui_overlay;
    bench.show_next_frame();
    let after = bench.backend.diagnostics().viewfinder.expect("видоискатель").ui_overlay;
    let frames = after.frames_drawn - before.frames_drawn;
    assert!(frames >= 2, "новый кадр и перекомпоновка нарисованы");
    assert_eq!(
        after.live_images_drawn - before.live_images_drawn,
        frames,
        "картинка прибора нарисована на каждом кадре UI, и на перекомпоновке тоже"
    );
    assert_eq!(after.live_images_missing, before.live_images_missing);
    assert_eq!(after.frames_failed, 0);
    assert!(scopes_of(&mut bench).counters.picture_handoffs >= frames);

    apply_scopes(&mut bench, ScopeIntent::Hide);
    bench.show_next_frame();
    let hidden_again = bench.backend.diagnostics().viewfinder.expect("видоискатель").ui_overlay;
    flush_frames(&mut bench, 2);
    let end = bench.backend.diagnostics().viewfinder.expect("видоискатель").ui_overlay;
    assert_eq!(
        end.live_images_drawn, hidden_again.live_images_drawn,
        "после «скрыть» рисовать нечего"
    );
    assert!(end.live_images_missing > hidden_again.live_images_missing);
    assert_eq!(end.frames_failed, 0);
    flush_frames(&mut bench, FRAMES_TO_FLUSH_SLOTS);
    assert_eq!(scopes_of(&mut bench).residency.resources, ScopeResources::Absent);
    bench.finish();
}

#[test]
fn a_source_leaving_withdraws_the_request_and_releases_the_scope_objects() {
    let scenes = vec![neutral_gray(); 6 * FRAMES_TO_FLUSH_SLOTS];
    let Some(mut bench) = start_without_ui(&scenes) else {
        return;
    };
    let mut expected_withdrawals = 0;

    // Установка нового источника при действующем запросе: свежий экран начинается без приборов прежнего.
    install(
        &mut bench,
        Box::new(ScopePictureSource::drawing_at_size_of(&waveform())),
    );
    assert_eq!(
        apply_scopes(&mut bench, ScopeIntent::Show(waveform())),
        ViewfinderCommandOutcome::Applied
    );
    assert_eq!(scopes_of(&mut bench).requested, Some(waveform()));
    install(
        &mut bench,
        Box::new(ScopePictureSource::drawing_at_size_of(&waveform())),
    );
    expected_withdrawals += 1;
    let after_install = scopes_of(&mut bench);
    assert_eq!(
        (after_install.requested, after_install.withdrawals),
        (None, expected_withdrawals)
    );

    // Снятие источника.
    apply_scopes(&mut bench, ScopeIntent::Show(waveform()));
    flush_frames(&mut bench, 2);
    assert_eq!(scopes_of(&mut bench).residency.resources, ScopeResources::Live);
    assert_eq!(
        bench.backend.apply_ui_overlay_command(UiOverlayCommand::Remove),
        UiOverlayOutcome::Applied
    );
    expected_withdrawals += 1;
    flush_frames(&mut bench, FRAMES_TO_FLUSH_SLOTS);
    let after_remove = scopes_of(&mut bench);
    assert_eq!(
        (after_remove.requested, after_remove.withdrawals),
        (None, expected_withdrawals)
    );
    assert_eq!(
        after_remove.residency.resources,
        ScopeResources::Absent,
        "объекты освобождены"
    );
    assert_eq!(
        after_remove.counters.resources_created,
        after_remove.counters.resources_released
    );

    // Замена источника новым.
    install(
        &mut bench,
        Box::new(ScopePictureSource::drawing_at_size_of(&waveform())),
    );
    apply_scopes(&mut bench, ScopeIntent::Show(waveform()));
    flush_frames(&mut bench, 2);
    install(
        &mut bench,
        Box::new(ScopePictureSource::drawing_at_size_of(&waveform())),
    );
    expected_withdrawals += 1;
    assert_eq!(
        scopes_of(&mut bench).requested,
        None,
        "замена отзывает запрос прежнего экрана"
    );

    // Паника источника: рендер снимает его сам, запрос отзывается вместе с ним.
    install(&mut bench, Box::new(PanickingSource));
    apply_scopes(&mut bench, ScopeIntent::Show(waveform()));
    flush_frames(&mut bench, 2);
    expected_withdrawals += 1;
    flush_frames(&mut bench, FRAMES_TO_FLUSH_SLOTS);
    let after_panic = scopes_of(&mut bench);
    assert_eq!(
        (after_panic.requested, after_panic.withdrawals),
        (None, expected_withdrawals)
    );
    assert_eq!(after_panic.residency.resources, ScopeResources::Absent);
    assert_eq!(
        after_panic.counters.resources_created,
        after_panic.counters.resources_released
    );
    bench.finish();
}

#[test]
fn hidden_scope_objects_are_retired_first_and_destroyed_only_after_their_slots_are_collected() {
    let scenes = vec![neutral_gray(); 6];
    let Some(mut bench) = start_without_ui(&scenes) else {
        return;
    };
    // С источником UI: кадр перекомпоновки показывает картинку прибора, и его слот держит набор без записи прохода.
    install(
        &mut bench,
        Box::new(ScopePictureSource::drawing_at_size_of(&waveform())),
    );
    apply_scopes(&mut bench, ScopeIntent::Show(waveform()));
    flush_frames(&mut bench, 3);
    assert_eq!(scopes_of(&mut bench).residency.resources, ScopeResources::Live);
    let passes = scopes_of(&mut bench).counters.passes_recorded;
    apply_scopes(&mut bench, ScopeIntent::Hide);

    // Кадры перекомпоновки по одному: объекты выводятся сразу и не уничтожаются, пока слоты кадров, которые их
    // читали (записали проход или показали картинку), не собраны после fence.
    let mut seen = Vec::new();
    for _ in 0..FRAMES_TO_FLUSH_SLOTS {
        bench.present_one_recompose_frame();
        let diagnostics = scopes_of(&mut bench);
        assert_ne!(
            diagnostics.residency.resources,
            ScopeResources::Live,
            "после «скрыть» набор не действует"
        );
        assert_eq!(
            diagnostics.counters.passes_recorded, passes,
            "перекомпоновка проходов не пишет"
        );
        seen.push(diagnostics.residency.resources);
    }
    assert_eq!(
        seen.first(),
        Some(&ScopeResources::Retiring),
        "первый кадр после «скрыть»: набор выведен, но слоты прежних кадров ещё не собраны: {seen:?}"
    );
    assert_eq!(seen.last(), Some(&ScopeResources::Absent), "потом уничтожен: {seen:?}");
    let diagnostics = scopes_of(&mut bench);
    assert_eq!(
        (
            diagnostics.counters.resources_created,
            diagnostics.counters.resources_released
        ),
        (1, 1)
    );
    bench.finish();
}

#[test]
fn every_nth_frame_computes_the_first_frame_then_every_nth_and_counts_the_skipped() {
    let mut render_config = config();
    render_config.viewfinder.scopes.every_nth_frame = 2;
    let scenes = [neutral_gray(), reddish(), bluish(), neutral_gray()];
    let Some(mut bench) = ViewfinderBench::start_with_scenes(&render_config, &scenes) else {
        return;
    };
    install(&mut bench, Box::new(IdleSource));
    let scope = waveform();
    apply_scopes(&mut bench, ScopeIntent::Show(scope));
    let mut recorded_after = Vec::new();
    for _ in &scenes {
        bench.show_next_frame();
        recorded_after.push(scopes_of(&mut bench).counters.passes_recorded);
    }
    assert_eq!(recorded_after, [1, 1, 2, 2], "кадры 1 и 3 считаются, 2 и 4 пропущены");
    assert_eq!(scopes_of(&mut bench).counters.frames_skipped_by_cadence, 2);
    assert!(gpu_counters(&bench).total() > 0, "последний посчитанный кадр измерен");
    bench.finish();
}
