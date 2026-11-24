//! Пропуск тиков без изменений (решение 214, сессия 16a) через НАСТОЯЩИЙ бэкенд показа: камера, ISP, UI со стеклом,
//! цепочка показа на реальном драйвере (headless, полная validation). Экран 120 Гц при камере 60 к/с: тики идут
//! парами «новый кадр камеры» + «кадра нет».
//!
//! Что доказывается (всегда по исходу `render_and_present`, а не по вызову):
//! * статичный UI: тик без нового кадра камеры возвращает `SkippedUnchanged`, тик с новым кадром — `Presented` на
//!   ТОМ ЖЕ тике (кадр камеры не опаздывает), каждый кадр камеры обработан ровно один раз;
//! * источник UI вызывается на КАЖДОМ тике, в том числе на пропущенном (касания и анимации идут как раньше);
//! * каждое условие, запрещающее пропуск, даёт `Presented` на том же тике: меняющийся UI (палец тянет ручку),
//!   затухающая анимация (пропуски начинаются сразу после покоя), свет от наклона, картинка (версия), установка и
//!   снятие источника, команды видоискателя, подавление работ GPU, отказ источника, предел серии, пересоздание цепочки;
//! * политика `AlwaysRedraw` — пропусков ноль;
//! * придержанная картинка swapchain переживает пересоздание и отсоединение без ошибок validation и утечек;
//! * общий «оракул»: на случайном сценарии тик пропускается тогда и только тогда, когда независимый подсчёт говорит
//!   «нового кадра нет, UI тот же, команд не было, серия не исчерпана».
//!
//! Пиксели итогового экрана здесь прочитать нельзя (у headless-цели нет чтения): равенство пикселей повторной
//! перекомпоновки доказывает GPU-тест на читаемой цели (`pipelines/ui_pass/unchanged_pixels_tests.rs`).

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod camera_rig;
mod support;
mod ui_overlay_rig;
mod vulkan_support;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use render_loop::backend::{DisplayBackend, PresentOutcome};
use render_loop::config::{RenderLoopConfig, UnchangedFrameConfig, UnchangedFramePolicy};
use render_loop::ui_overlay::{UiOverlayCommand, UiOverlayOutcome};
use render_loop::viewfinder::{FramingDestination, NormalizedScreenRect, ViewfinderFraming};
use render_loop::viewfinder::{
    GpuWorkSuppression, SuppressibleGpuWork, UnchangedFrameCounters, ViewfinderCommand, ViewfinderCommandOutcome,
};
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{DrawItem, DrawList, ShapeItem};
use ui_draw::frame_source::{UiFrameError, UiFrameRequest, UiFrameSource};
use ui_draw::light::LightDirection;
use ui_draw::shape::ShapeGeometry;
use ui_draw::transform::Affine2;
use ui_overlay_rig::{Harness, PictureProbe, RESIZED_SURFACE, SURFACE, SourceProbe, test_picture};

const SKIPPED: PresentOutcome = PresentOutcome::SkippedUnchanged;
const PRESENTED: PresentOutcome = PresentOutcome::Presented;

// ---- стенд ----

/// Источник со списком, которым управляет тест, и счётчиком вызовов.
struct ScriptedSource {
    list: Arc<Mutex<DrawList>>,
    calls: Arc<AtomicU64>,
}

impl UiFrameSource for ScriptedSource {
    fn name(&self) -> &'static str {
        "scripted-unchanged-test"
    }

    fn produce_frame(&mut self, _request: &UiFrameRequest) -> Result<DrawList, UiFrameError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.list.lock().unwrap_or_else(PoisonError::into_inner).clone())
    }
}

/// Управляемый тестом список рисования.
struct Script {
    list: Arc<Mutex<DrawList>>,
    calls: Arc<AtomicU64>,
}

impl Script {
    fn new() -> Self {
        Self {
            list: Arc::new(Mutex::new(shapes(45.0, 2))),
            calls: Arc::new(AtomicU64::new(0)),
        }
    }

    fn set(&self, list: DrawList) {
        *self.list.lock().unwrap_or_else(PoisonError::into_inner) = list;
    }

    fn calls(&self) -> u64 {
        self.calls.load(Ordering::SeqCst)
    }

    fn install_into(&self, harness: &mut Harness) {
        let source = ScriptedSource {
            list: Arc::clone(&self.list),
            calls: Arc::clone(&self.calls),
        };
        assert_eq!(
            harness
                .backend
                .apply_ui_overlay_command(UiOverlayCommand::Install(Box::new(source))),
            UiOverlayOutcome::Applied
        );
    }
}

/// Список из `count` кругов с углом света `light_degrees`.
fn shapes(light_degrees: f32, count: usize) -> DrawList {
    let mut list = DrawList::new(LightDirection::from_angle_degrees(light_degrees));
    for index in 0..count {
        list.items.push(DrawItem::Shape(ShapeItem {
            geometry: ShapeGeometry::Circle { diameter: 40.0 },
            transform: Affine2::translate(60.0 + 50.0 * index as f32, 100.0),
            fill: DisplayColor {
                r: 0.9,
                g: 0.4,
                b: 0.2,
                a: 1.0,
            },
            stroke: None,
            shadow: None,
            opacity: Opacity::OPAQUE,
        }));
    }
    list
}

fn skipping_config() -> RenderLoopConfig {
    RenderLoopConfig {
        unchanged_frame: UnchangedFrameConfig {
            policy: UnchangedFramePolicy::SkipWhenUnchanged,
            ..UnchangedFrameConfig::default()
        },
        ..RenderLoopConfig::default()
    }
}

fn start() -> Option<Harness> {
    start_with(skipping_config())
}

fn start_with(config: RenderLoopConfig) -> Option<Harness> {
    let mut harness = Harness::start_skipping_unchanged(config)?;
    harness.attach(1, SURFACE);
    Some(harness)
}

fn counters(harness: &mut Harness) -> UnchangedFrameCounters {
    harness.viewfinder().counters.unchanged_frames
}

fn camera_diagnostics(harness: &mut Harness) -> render_loop::camera_diagnostics::CameraStreamDiagnostics {
    harness.backend.diagnostics().camera.expect("диагностика показа камеры")
}

/// Новый кадр камеры и тик показа: возвращает исход тика.
fn tick_with_new_camera_frame(harness: &mut Harness) -> PresentOutcome {
    harness.offer_camera_frame();
    harness.tick()
}

/// Прогрев: кадры камеры идут, пока ISP не обработал хотя бы один и экран не вышел на устойчивый ритм
/// «кадр камеры — перекомпоновка». Возвращает число обработанных кадров.
fn warm_up(harness: &mut Harness) -> u64 {
    for _ in 0..4 {
        assert_eq!(
            tick_with_new_camera_frame(harness),
            PRESENTED,
            "кадр камеры рисуется сразу"
        );
        let _ = harness.tick();
    }
    harness.viewfinder().counters.frames_processed
}

// ---- 120 Гц при камере 60 к/с ----

#[test]
fn at_120hz_with_a_60fps_camera_every_second_tick_is_skipped_and_no_camera_frame_is_late() {
    let Some(mut harness) = start() else { return };
    let script = Script::new();
    script.install_into(&mut harness);
    // Холодный старт и первые кадры: до устойчивого ритма.
    let processed_before = warm_up(&mut harness);
    let before = counters(&mut harness);
    let calls_before = script.calls();
    let camera_before = camera_diagnostics(&mut harness);

    const PAIRS: u64 = 30;
    for pair in 0..PAIRS {
        assert_eq!(
            tick_with_new_camera_frame(&mut harness),
            PRESENTED,
            "пара {pair}: тик с новым кадром камеры рисуется на том же тике"
        );
        assert_eq!(
            harness.tick(),
            SKIPPED,
            "пара {pair}: тик без нового кадра и без изменений пропущен"
        );
    }

    let after = counters(&mut harness);
    assert_eq!(after.skipped - before.skipped, PAIRS, "ровно по пропуску на пару");
    assert_eq!(
        after.drawn_new_camera_frame - before.drawn_new_camera_frame,
        PAIRS,
        "рисуется каждый новый кадр камеры"
    );
    assert_eq!(
        harness.viewfinder().counters.frames_processed - processed_before,
        PAIRS,
        "каждый кадр камеры обработан ровно один раз"
    );
    assert_eq!(
        script.calls() - calls_before,
        2 * PAIRS,
        "источник UI спрошен на каждом тике, пропущенные включены"
    );
    // Метрика «видимые повторы» не зависит от пропуска: повтор, который не нарисован, всё равно виден зрителю.
    let camera_after = camera_diagnostics(&mut harness);
    assert_eq!(
        camera_after.camera_frame_repeats_held - camera_before.camera_frame_repeats_held,
        PAIRS
    );
    assert_eq!(
        camera_after.camera_frame_redraws, camera_before.camera_frame_redraws,
        "ничего не перерисовано"
    );
    assert_eq!(camera_after.visible_repeats() - camera_before.visible_repeats(), PAIRS);
    harness.finish();
}

#[test]
fn a_ui_that_changes_on_every_tick_is_never_skipped() {
    let Some(mut harness) = start() else { return };
    let script = Script::new();
    script.install_into(&mut harness);
    warm_up(&mut harness);
    let before = counters(&mut harness);

    // Палец двигает ручку: свет (и положение) другие на каждом тике, камера молчит.
    for step in 0..20 {
        script.set(shapes(10.0 + step as f32, 2));
        assert_eq!(
            harness.tick(),
            PRESENTED,
            "тик {step}: UI изменился — рисуется на том же тике"
        );
    }
    let after = counters(&mut harness);
    assert_eq!(
        after.skipped, before.skipped,
        "ни одного пропуска под движущимся пальцем"
    );
    assert_eq!(after.drawn_ui_changed - before.drawn_ui_changed, 20);
    harness.finish();
}

#[test]
fn after_an_animation_settles_skipping_starts_on_the_very_next_tick() {
    let Some(mut harness) = start() else { return };
    let script = Script::new();
    script.install_into(&mut harness);
    warm_up(&mut harness);

    // Пружина: шаги затухают 3 тика, потом покой.
    let mut outcomes = Vec::new();
    for light in [50.0, 47.0, 46.0, 46.0, 46.0, 46.0] {
        script.set(shapes(light, 2));
        outcomes.push(harness.tick());
    }
    assert_eq!(
        outcomes,
        [PRESENTED, PRESENTED, PRESENTED, SKIPPED, SKIPPED, SKIPPED],
        "три тика движения рисуются, первый тик покоя уже пропущен"
    );
    harness.finish();
}

#[test]
fn light_from_tilt_a_new_item_and_a_new_picture_version_each_force_a_draw() {
    let Some(mut harness) = start() else { return };
    let script = Script::new();
    script.install_into(&mut harness);
    warm_up(&mut harness);
    assert_eq!(harness.tick(), SKIPPED, "исходное состояние: покой");

    script.set(shapes(46.0, 2));
    assert_eq!(harness.tick(), PRESENTED, "свет изменился (наклон телефона)");
    assert_eq!(harness.tick(), SKIPPED);

    script.set(shapes(46.0, 3));
    assert_eq!(harness.tick(), PRESENTED, "появился элемент");
    assert_eq!(harness.tick(), SKIPPED);
    harness.finish();
}

#[test]
fn a_picture_forces_a_draw_only_when_its_version_changes() {
    let Some(mut harness) = start() else { return };
    let probe = PictureProbe::new();
    harness.install_pictures(&probe);
    let thumbnail = test_picture(16, 12);
    probe.show(Some(thumbnail.clone()));
    warm_up(&mut harness);
    assert_eq!(harness.tick(), SKIPPED, "та же версия картинки — ничего не изменилось");

    // Те же пиксели, но новая версия (новая миниатюра снимка): рисуется.
    probe.show(Some(test_picture(16, 12)));
    assert_eq!(harness.tick(), PRESENTED, "новая версия картинки");
    assert_eq!(harness.tick(), SKIPPED);

    probe.show(None);
    assert_eq!(harness.tick(), PRESENTED, "картинка исчезла");
    assert_eq!(harness.tick(), SKIPPED);
    harness.finish();
}

#[test]
fn installing_and_removing_the_ui_source_force_a_draw_on_the_same_tick() {
    let Some(mut harness) = start() else { return };
    warm_up(&mut harness);
    assert_eq!(
        harness.tick(),
        SKIPPED,
        "без источника UI тик без изменений тоже пропускается"
    );

    let script = Script::new();
    script.install_into(&mut harness);
    assert_eq!(harness.tick(), PRESENTED, "появился источник UI");
    assert_eq!(harness.tick(), SKIPPED);

    assert_eq!(harness.remove(), UiOverlayOutcome::Applied);
    assert_eq!(harness.tick(), PRESENTED, "источник снят — UI исчезает с экрана");
    assert_eq!(harness.tick(), SKIPPED);
    harness.finish();
}

#[test]
fn a_failing_source_is_never_skipped_and_recovers_to_skipping_after_it_works_again() {
    let Some(mut harness) = start() else { return };
    let probe = SourceProbe::new();
    harness.install(&probe);
    warm_up(&mut harness);
    assert_eq!(harness.tick(), SKIPPED);

    probe.failing.store(true, Ordering::SeqCst);
    for _ in 0..3 {
        assert_eq!(
            harness.tick(),
            PRESENTED,
            "кадр UI не получен — состояние экрана неизвестно"
        );
    }
    assert!(counters(&mut harness).drawn_ui_unavailable >= 3);

    probe.failing.store(false, Ordering::SeqCst);
    assert_eq!(harness.tick(), PRESENTED, "первый кадр после сбоя: эталона нет");
    assert_eq!(harness.tick(), SKIPPED);
    harness.finish();
}

#[test]
fn a_viewfinder_command_forces_a_draw_and_gpu_work_suppression_blocks_skipping() {
    let Some(mut harness) = start() else { return };
    let script = Script::new();
    script.install_into(&mut harness);
    warm_up(&mut harness);
    assert_eq!(harness.tick(), SKIPPED);

    // Команда видоискателя, не меняющая UI: масштаб экрана в подавление не входит, но версия сцены растёт.
    let ui_only = GpuWorkSuppression::NONE.suppressing(SuppressibleGpuWork::ColorScopes);
    assert_eq!(
        harness
            .backend
            .apply_viewfinder_command(ViewfinderCommand::SuppressGpuWork(ui_only)),
        ViewfinderCommandOutcome::Applied
    );
    for step in 0..3 {
        assert_eq!(
            harness.tick(),
            PRESENTED,
            "под подавлением работ тик {step} рисуется: замер A/B сравним"
        );
    }
    let suppressed = counters(&mut harness);
    assert!(suppressed.drawn_gpu_work_suppressed >= 3, "{suppressed:?}");

    assert_eq!(
        harness
            .backend
            .apply_viewfinder_command(ViewfinderCommand::SuppressGpuWork(GpuWorkSuppression::NONE)),
        ViewfinderCommandOutcome::Applied
    );
    assert_eq!(
        harness.tick(),
        PRESENTED,
        "подавление снято: кадр после подавления эталоном не был"
    );
    assert_eq!(harness.tick(), SKIPPED, "дальше покой снова пропускается");
    harness.finish();
}

#[test]
fn a_framing_command_moves_the_viewfinder_so_the_next_tick_is_drawn_even_with_the_same_ui() {
    let Some(mut harness) = start() else { return };
    let script = Script::new();
    script.install_into(&mut harness);
    warm_up(&mut harness);
    assert_eq!(harness.tick(), SKIPPED);
    let before = counters(&mut harness);

    // Видоискатель переехал в нижнюю половину экрана: UI тот же, а пиксели другие.
    let lower_half = NormalizedScreenRect::new(0.0, 0.5, 1.0, 0.5).expect("прямоугольник");
    let framing = ViewfinderFraming {
        crop: render_loop::viewfinder::FramingCrop::Fixed(camera_contract::viewfinder_crop::ViewfinderCrop {
            aspect: camera_contract::viewfinder_crop::CropAspect::SQUARE,
            zoom: camera_contract::viewfinder_crop::ZoomFactor::NONE,
            center: camera_contract::viewfinder_crop::CropCenter::ValidAreaCenter,
        }),
        destination: FramingDestination::ScreenRect(lower_half),
    };
    assert_eq!(
        harness
            .backend
            .apply_viewfinder_command(ViewfinderCommand::SetFraming(framing)),
        ViewfinderCommandOutcome::Applied
    );
    assert_eq!(
        harness.tick(),
        PRESENTED,
        "команда кадрирования: тик рисуется на том же тике"
    );
    assert_eq!(harness.tick(), SKIPPED, "дальше покой");
    assert_eq!(
        counters(&mut harness).drawn_scene_changed - before.drawn_scene_changed,
        1
    );
    harness.finish();
}

#[test]
fn the_skip_run_is_cut_by_the_configured_limit() {
    let config = RenderLoopConfig {
        unchanged_frame: UnchangedFrameConfig {
            policy: UnchangedFramePolicy::SkipWhenUnchanged,
            max_consecutive_skips: 2,
        },
        ..RenderLoopConfig::default()
    };
    let Some(mut harness) = start_with(config) else { return };
    let script = Script::new();
    script.install_into(&mut harness);
    warm_up(&mut harness);
    // Начинаем серию с нарисованного кадра: прогрев мог кончиться пропуском.
    script.set(shapes(61.0, 2));
    assert_eq!(harness.tick(), PRESENTED);

    let outcomes: Vec<_> = (0..7).map(|_| harness.tick()).collect();
    assert_eq!(
        outcomes,
        [SKIPPED, SKIPPED, PRESENTED, SKIPPED, SKIPPED, PRESENTED, SKIPPED],
        "после двух пропусков подряд кадр рисуется (нужен слот кольца для итогов кадров)"
    );
    assert!(counters(&mut harness).drawn_skip_run_limit >= 2);
    harness.finish();
}

#[test]
fn always_redraw_policy_never_skips() {
    let config = RenderLoopConfig {
        unchanged_frame: UnchangedFrameConfig {
            policy: UnchangedFramePolicy::AlwaysRedraw,
            ..UnchangedFrameConfig::default()
        },
        ..RenderLoopConfig::default()
    };
    let Some(mut harness) = start_with(config) else { return };
    let script = Script::new();
    script.install_into(&mut harness);
    warm_up(&mut harness);
    for step in 0..10 {
        assert_eq!(harness.tick(), PRESENTED, "тик {step}");
    }
    let counters = counters(&mut harness);
    assert_eq!(counters.skipped, 0);
    assert!(counters.drawn_policy_always_redraw >= 10, "{counters:?}");
    assert_eq!(
        camera_diagnostics(&mut harness).camera_frame_repeats_held,
        0,
        "пропусков не было"
    );
    harness.finish();
}

// ---- придержанная картинка и цепочка показа ----

#[test]
fn resizing_the_chain_while_a_skipped_tick_holds_an_image_recovers_and_draws_first() {
    let Some(mut harness) = start() else { return };
    let script = Script::new();
    script.install_into(&mut harness);
    warm_up(&mut harness);
    assert_eq!(harness.tick(), SKIPPED, "картинка придержана");

    harness.backend.resize(RESIZED_SURFACE).expect("пересоздание цепочки");
    assert_eq!(
        harness.tick(),
        PRESENTED,
        "после пересоздания на экране ничего не показано: первый тик рисуется (эталона нет)"
    );
    assert_eq!(harness.tick(), SKIPPED, "дальше покой снова пропускается");
    harness.finish();
}

#[test]
fn detaching_while_a_skipped_tick_holds_an_image_leaves_no_objects_and_no_validation_errors() {
    let Some(mut harness) = start() else { return };
    let script = Script::new();
    script.install_into(&mut harness);
    warm_up(&mut harness);
    assert_eq!(harness.tick(), SKIPPED, "картинка придержана");

    // `finish` отсоединяет surface и проверяет, что объекты Vulkan вернулись к исходным, а validation молчит.
    harness.finish();
}

#[test]
fn resuming_after_a_pause_forgets_the_shown_frame_and_draws_the_first_tick() {
    let Some(mut harness) = start() else { return };
    let script = Script::new();
    script.install_into(&mut harness);
    warm_up(&mut harness);
    assert_eq!(harness.tick(), SKIPPED, "картинка придержана");
    harness.backend.presentation_resumed();
    assert_eq!(
        harness.tick(),
        PRESENTED,
        "после возврата что на экране, неизвестно: тик рисуется"
    );
    assert_eq!(harness.tick(), SKIPPED, "дальше покой снова пропускается");
    harness.finish();
}

#[test]
fn a_new_surface_starts_without_a_shown_frame_and_draws_first() {
    let Some(mut harness) = start() else { return };
    let script = Script::new();
    script.install_into(&mut harness);
    warm_up(&mut harness);
    assert_eq!(harness.tick(), SKIPPED);
    harness.detach();

    harness.attach(2, SURFACE);
    // Новая цепочка: изображение ISP прежней сессии пропало вместе с ней, на экране нечего перекомпоновывать.
    assert_eq!(
        harness.tick(),
        PRESENTED,
        "на новой поверхности показывать нечего: тик рисуется"
    );
    assert_eq!(
        tick_with_new_camera_frame(&mut harness),
        PRESENTED,
        "первый кадр камеры на новой поверхности рисуется сразу"
    );
    assert_eq!(harness.tick(), SKIPPED);
    harness.finish();
}

// ---- оракул ----

/// Следующее псевдослучайное число (xorshift): сценарий воспроизводим без внешних крейтов.
fn next_random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// Независимый подсчёт: тик можно пропустить только если нового кадра камеры нет, список тот же, что в последнем
/// показанном кадре, и серия пропусков не исчерпана. Бэкенд обязан совпасть с ним на каждом тике.
#[test]
fn on_a_random_scenario_a_tick_is_skipped_exactly_when_nothing_could_have_changed() {
    const LIMIT: u32 = 3;
    let config = RenderLoopConfig {
        unchanged_frame: UnchangedFrameConfig {
            policy: UnchangedFramePolicy::SkipWhenUnchanged,
            max_consecutive_skips: LIMIT,
        },
        ..RenderLoopConfig::default()
    };
    let Some(mut harness) = start_with(config) else { return };
    let script = Script::new();
    script.install_into(&mut harness);
    warm_up(&mut harness);
    // Прогрев закончился тихим тиком: эталон — текущий список.
    assert_eq!(harness.tick(), SKIPPED);

    let mut shown = script.list.lock().expect("список").clone();
    let mut run = 1_u32;
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let (mut skipped, mut drawn) = (0, 0);
    for step in 0..200 {
        let roll = next_random(&mut state) % 10;
        let new_camera_frame = roll < 3;
        let changes_ui = (3..5).contains(&roll);
        if changes_ui {
            script.set(shapes(
                20.0 + (next_random(&mut state) % 5) as f32,
                1 + (next_random(&mut state) % 3) as usize,
            ));
        }
        if new_camera_frame {
            harness.offer_camera_frame();
        }
        let current = script.list.lock().expect("список").clone();
        let expect_skip = !new_camera_frame && current == shown && run < LIMIT;
        let outcome = harness.tick();
        assert_eq!(
            outcome,
            if expect_skip { SKIPPED } else { PRESENTED },
            "шаг {step}: новый кадр {new_camera_frame}, UI изменён {changes_ui}, серия {run}"
        );
        if expect_skip {
            run += 1;
            skipped += 1;
        } else {
            run = 0;
            shown = current;
            drawn += 1;
        }
    }
    assert!(
        skipped > 20 && drawn > 20,
        "сценарий задел обе ветки: пропущено {skipped}, нарисовано {drawn}"
    );
    harness.finish();
}
