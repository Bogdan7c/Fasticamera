//! Сквозной тест цветовых приборов на ПК (сессия 14h, решения 183, 189, 190): действие экрана
//! `ScreenAction::Scopes` → редьюсер `camera-screen` → исполнитель приборов приложения (`ScopeEffects`) → команда
//! рендеру → НАСТОЯЩИЙ бэкенд показа (RADV, headless, полная validation, подставная камера) с установленным
//! источником UI, который рисует картинку прибора.
//!
//! Что доказывается:
//! * счётчики GPU равны эталону `scope_model::accumulate` по картинке видоискателя ТОГО ЖЕ кадра (решение 190:
//!   сюжеты кадров разные, поэтому «мерил другой кадр» провалило бы тест);
//! * картинка прибора на GPU совпадает с `scope_model::render` в допуске fp16 и доходит до рисования UI
//!   (элемент «живая картинка» нарисован, а не пропущен);
//! * «скрыть» тем же путём: проходов больше нет, объекты GPU освобождены, UI пропускает элемент без сбоя;
//! * оси вектороскопа (OKLab против Cb/Cr) доходят до GPU: счётчики по OKLab равны эталону по OKLab и не равны
//!   эталону по Cb/Cr.
//!
//! Чтения готового экранного буфера (после компоновки UI поверх видоискателя) в стендах нет, а добавлять для него
//! тестовый шов в продакшен-код ради одного теста не стали: вместо «пиксели в прямоугольнике» проверяются картинка
//! прибора (чтение `read_scope_picture`) и счёт нарисованных элементов UI.
//!
//! Стенд подставной камеры — тот же файл, что у тестов `render-loop`, подключён по пути. «Ящик» рендера здесь —
//! запись команд: их применяет сам тест тем же `apply_viewfinder_command`, каким пользуется поток рендера.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

#[path = "../../render-loop/tests/support/mod.rs"]
mod support;
#[path = "../../render-loop/tests/viewfinder_rig/mod.rs"]
mod viewfinder_rig;
#[path = "../../render-loop/tests/vulkan_support/mod.rs"]
mod vulkan_support;

use std::sync::{Arc, Mutex};

use camera_screen::{Effect, ScreenAction, ScreenConfig, ScreenState, reduce};
use fasticamera::scope_effects::{ScopeCounters as DeliveryCounters, ScopeEffects};
use fasticamera::viewfinder_control::{ControlDelivery, RenderAdjustment, RenderControl};
use render_loop::backend::DisplayBackend;
use render_loop::config::{DisplayColorConfig, RenderLoopConfig, ScopesConfig, ViewfinderConfig};
use render_loop::ui_overlay::{UiOverlayCommand, UiOverlayOutcome};
use render_loop::viewfinder::{ScopeDiagnostics, ScopeResources, ViewfinderCommandOutcome};
use scope_contract::{HistogramMode, ScopeImageSize, ScopeIntent, ScopeKind, ScopeRequest, VectorscopeAxes};
use scope_model::{
    HorizontalOrder, ResolvedScope, SampleCount, SampleStride, ScopeCounters, ScopeModelSettings, ScopeRule,
    ShownCodes, accumulate, render,
};
use ui_draw::color::Opacity;
use ui_draw::draw_list::{DrawItem, DrawList, ImageClip, ImageFit, ImageItem, ImageSource, LiveImage};
use ui_draw::frame_source::{UiFrameError, UiFrameRequest, UiFrameSource};
use ui_draw::geometry::PxSize;
use ui_draw::light::LightDirection;
use ui_draw::transform::Affine2;
use viewfinder_rig::{CellValues, ViewfinderBench, neutral_gray, reddish};

/// Допуск картинки прибора: премультиплицированные значения, четыре шага fp16 у единицы (как в тестах `isp-vulkan`).
const PICTURE_TOLERANCE: f32 = 2.0e-3;
/// Кадров, гарантированно достаточно, чтобы все слоты кольца были собраны после fence.
const FRAMES_TO_FLUSH_SLOTS: usize = 8;

/// Ящик рендера теста: запоминает команды, как поток рендера запоминает их до применения.
#[derive(Default)]
struct RecordingRenderInbox {
    adjustments: Mutex<Vec<RenderAdjustment>>,
}

impl RenderControl for RecordingRenderInbox {
    fn adjust(&self, adjustment: RenderAdjustment) -> ControlDelivery {
        self.adjustments.lock().expect("замок").push(adjustment);
        ControlDelivery::Delivered
    }

    fn ui_overlay(&self, _command: UiOverlayCommand) -> ControlDelivery {
        ControlDelivery::Delivered
    }
}

/// Источник UI, рисующий картинку прибора по центру (так её будет рисовать раздел калибровки в 14b): элемент
/// ровно размером картинки, как у экрана (иначе рендер не отдаст картинку: см. `scope_fit` в render-loop).
struct ScopePictureSource {
    /// Размер, которым экран рисует элемент прямо сейчас (тест меняет его, как смена прибора меняет размер зоны).
    picture_size: Arc<Mutex<ScopeImageSize>>,
}

impl UiFrameSource for ScopePictureSource {
    fn name(&self) -> &'static str {
        "scopes-e2e"
    }

    fn produce_frame(&mut self, request: &UiFrameRequest) -> Result<DrawList, UiFrameError> {
        let picture_size = *self.picture_size.lock().expect("замок");
        let mut list = DrawList::new(LightDirection::from_angle_degrees(45.0));
        list.items.push(DrawItem::Image(ImageItem {
            source: ImageSource::Live(LiveImage::ColorScope),
            transform: Affine2::translate(request.logical_size.width * 0.5, request.logical_size.height * 0.5),
            size_px: PxSize::new(picture_size.width() as f32, picture_size.height() as f32),
            fit: ImageFit::Stretch,
            clip: ImageClip::None,
            opacity: Opacity::OPAQUE,
        }));
        Ok(list)
    }
}

/// Модель приборов поменьше (тест не гоняет эталон по 256×256×3), шаг 1: меряется каждый пиксель.
fn small_model() -> ScopeModelSettings {
    ScopeModelSettings {
        trace_max_columns: 48,
        trace_levels: 64,
        vectorscope_size: 64,
        sample_stride: SampleStride::try_new(1).expect("шаг"),
        ..ScopeModelSettings::default()
    }
}

fn config() -> RenderLoopConfig {
    RenderLoopConfig {
        display_color: DisplayColorConfig::SrgbOnly,
        viewfinder: ViewfinderConfig {
            scopes: ScopesConfig {
                // Каждый кадр: тест сверяет прибор с ПОСЛЕДНИМ показанным кадром (каденция — отдельные тесты).
                every_nth_frame: 1,
                model: small_model(),
                ..ScopesConfig::default()
            },
            ..ViewfinderConfig::default()
        },
        ..RenderLoopConfig::default()
    }
}

fn request(kind: ScopeKind, axes: VectorscopeAxes) -> ScopeRequest {
    ScopeRequest {
        kind,
        vectorscope_axes: axes,
        vectorscope_zoom: scope_contract::VectorscopeZoom::Normal,
        cie_diagram: scope_contract::CieDiagram::default(),
        histogram_mode: scope_contract::HistogramMode::Rgb,
        scan: scope_contract::ScopeScan::Upright,
        image_size: ScopeImageSize::try_new(160, 96).expect("размер"),
    }
}

/// Синеватый сюжет: третий, отличный от серого и красноватого.
fn bluish() -> CellValues {
    CellValues {
        red: 220,
        green: 300,
        blue: 620,
    }
}

/// Стенд приложения: экран → исполнитель приборов → ящик рендера → бэкенд.
struct ScreenToRender {
    screen_config: ScreenConfig,
    state: ScreenState,
    scopes: ScopeEffects,
    inbox: RecordingRenderInbox,
}

impl ScreenToRender {
    fn new() -> Self {
        let screen_config = ScreenConfig::default();
        let state = ScreenState::new(&screen_config).expect("состояние экрана");
        Self {
            screen_config,
            state,
            scopes: ScopeEffects::default(),
            inbox: RecordingRenderInbox::default(),
        }
    }

    /// Действие экрана проходит настоящий редьюсер и исполнитель; команды рендеру копятся в ящике.
    fn dispatch(&mut self, intent: ScopeIntent) {
        let transition = reduce(&self.state, ScreenAction::Scopes(intent), &self.screen_config);
        assert_eq!(
            transition.effects,
            vec![Effect::Scopes(intent)],
            "ровно одно намерение наружу"
        );
        for effect in transition.effects {
            match effect {
                Effect::Scopes(intent) => self.scopes.run_intent(intent, &self.inbox),
                other => panic!("действие прибора породило посторонний эффект: {other:?}"),
            }
        }
    }

    /// Поток рендера забирает команды из ящика и применяет их; возвращает исходы применения.
    fn deliver_to(&self, bench: &mut ViewfinderBench) -> Vec<ViewfinderCommandOutcome> {
        let delivered: Vec<RenderAdjustment> = self.inbox.adjustments.lock().expect("замок").drain(..).collect();
        delivered
            .into_iter()
            .map(|adjustment| {
                let RenderAdjustment::Viewfinder(command) = adjustment else {
                    panic!("ожидалась команда видоискателю: {adjustment:?}");
                };
                bench.backend.apply_viewfinder_command(command)
            })
            .collect()
    }
}

fn scopes_of(bench: &mut ViewfinderBench) -> ScopeDiagnostics {
    bench
        .backend
        .diagnostics()
        .viewfinder
        .expect("диагностика видоискателя")
        .scopes
}

fn ui_report(bench: &mut ViewfinderBench) -> render_loop::ui_overlay::UiOverlayDiagnostics {
    bench.backend.diagnostics().viewfinder.expect("видоискатель").ui_overlay
}

fn settle(bench: &ViewfinderBench) {
    bench.backend.context().wait_idle().expect("wait_idle");
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

/// Эталон по картинке видоискателя, прочитанной с GPU (то, что показано на экране ЭТОГО кадра).
fn reference(bench: &ViewfinderBench, scope: &ScopeRequest) -> (ScopeCounters, ResolvedScope, SampleCount) {
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
    let counters = accumulate(
        &shown,
        rule.planned_samples(shown.width(), shown.height(), &model).steps(),
        &rule as &dyn ScopeRule,
    )
    .expect("эталон");
    let samples = rule.planned_samples(shown.width(), shown.height(), &model);
    (counters, rule, samples)
}

/// Наибольшее расхождение премультиплицированных RGBA между GPU и эталоном.
fn worst_premultiplied_difference(gpu: &[[f32; 4]], cpu: &[[f32; 4]]) -> f32 {
    assert_eq!(gpu.len(), cpu.len());
    let premultiplied = |pixel: [f32; 4]| [pixel[0] * pixel[3], pixel[1] * pixel[3], pixel[2] * pixel[3], pixel[3]];
    gpu.iter()
        .zip(cpu)
        .flat_map(|(gpu_pixel, cpu_pixel)| {
            let (gpu_value, cpu_value) = (premultiplied(*gpu_pixel), premultiplied(*cpu_pixel));
            (0..4).map(move |channel| (gpu_value[channel] - cpu_value[channel]).abs())
        })
        .fold(0.0, f32::max)
}

fn install_scope_screen(bench: &mut ViewfinderBench, picture_size: ScopeImageSize) -> Arc<Mutex<ScopeImageSize>> {
    let shared_size = Arc::new(Mutex::new(picture_size));
    let source = ScopePictureSource {
        picture_size: Arc::clone(&shared_size),
    };
    assert_eq!(
        bench
            .backend
            .apply_ui_overlay_command(UiOverlayCommand::Install(Box::new(source))),
        UiOverlayOutcome::Applied
    );
    shared_size
}

#[test]
fn a_screen_show_waveform_intent_gives_gpu_counters_of_the_shown_frame_and_a_drawn_picture_then_hide_releases_all() {
    let scenes = [neutral_gray(), reddish(), bluish(), reddish()];
    let mut all_scenes = scenes.to_vec();
    all_scenes.extend(vec![neutral_gray(); FRAMES_TO_FLUSH_SLOTS + 2]);
    let Some(mut bench) = ViewfinderBench::start_with_scenes(&config(), &all_scenes) else {
        return;
    };
    let scope = request(ScopeKind::Waveform, VectorscopeAxes::YCbCrBt709);
    let mut screen = ScreenToRender::new();

    // Порядок решения 14h: источник экрана ставится ПЕРВЫМ (его установка отзывает запросы), затем `Show`.
    install_scope_screen(&mut bench, scope.image_size);
    screen.dispatch(ScopeIntent::Show(scope));
    assert_eq!(screen.deliver_to(&mut bench), vec![ViewfinderCommandOutcome::Applied]);
    assert_eq!(scopes_of(&mut bench).requested, Some(scope));

    let mut previous: Option<ScopeCounters> = None;
    for shown in 1..=scenes.len() as u64 {
        bench.show_next_frame();
        let diagnostics = scopes_of(&mut bench);
        assert_eq!(
            diagnostics.counters.passes_recorded, shown,
            "один проход на каждый новый кадр"
        );
        let gpu = gpu_counters(&bench);
        let (cpu, rule, samples) = reference(&bench, &scope);
        assert_eq!(
            gpu, cpu,
            "кадр {shown}: счётчики GPU разошлись с эталоном по показанной картинке"
        );
        assert!(cpu.total() > 0, "кадр {shown}: прибор что-то сосчитал");
        if let Some(previous) = &previous {
            assert_ne!(previous, &cpu, "кадр {shown}: сюжет сменился, значит и счётчики другие");
        }
        previous = Some(cpu.clone());

        // Картинка прибора = эталон показа по тем же счётчикам.
        let picture = bench
            .backend
            .read_scope_picture()
            .expect("чтение картинки прибора")
            .expect("картинка есть");
        let expected = render(&cpu, &rule, &small_model().render, scope.image_size, samples).expect("эталон показа");
        let worst = worst_premultiplied_difference(&picture.texels, expected.pixels());
        assert!(
            worst <= PICTURE_TOLERANCE,
            "кадр {shown}: картинка прибора разошлась с эталоном на {worst}"
        );
        assert!(
            picture.texels.iter().filter(|pixel| pixel[3] > 0.0).count() > 100,
            "кадр {shown}: на картинке есть след и сетка"
        );
    }

    // UI реально нарисовал картинку прибора, а не пропустил: по одному элементу на каждый кадр UI.
    let drawn = ui_report(&mut bench);
    assert!(drawn.frames_drawn >= scenes.len() as u64);
    assert_eq!(
        drawn.live_images_drawn, drawn.frames_drawn,
        "картинка нарисована на каждом кадре UI"
    );
    assert_eq!(drawn.live_images_missing, 0, "ни одного пропуска, пока прибор показан");
    assert_eq!((drawn.frames_failed, drawn.source_failures), (0, 0));
    assert_eq!(scopes_of(&mut bench).residency.resources, ScopeResources::Live);

    // «Скрыть» тем же путём.
    screen.dispatch(ScopeIntent::Hide);
    assert_eq!(screen.deliver_to(&mut bench), vec![ViewfinderCommandOutcome::Applied]);
    let drawn_before_hide = ui_report(&mut bench);
    let passes = scopes_of(&mut bench).counters.passes_recorded;
    for _ in 0..FRAMES_TO_FLUSH_SLOTS + 2 {
        bench.present_one_recompose_frame();
    }
    let hidden = scopes_of(&mut bench);
    assert_eq!(hidden.counters.passes_recorded, passes, "после «скрыть» проходов нет");
    assert_eq!(hidden.requested, None);
    assert_eq!(hidden.residency.resources, ScopeResources::Absent);
    assert_eq!(
        (hidden.counters.resources_created, hidden.counters.resources_released),
        (1, 1),
        "объекты GPU приборов созданы и освобождены"
    );
    let after = ui_report(&mut bench);
    assert_eq!(
        after.live_images_drawn, drawn_before_hide.live_images_drawn,
        "картинку больше не рисуют"
    );
    assert!(
        after.live_images_missing > 0,
        "UI считает элемент пропущенным: {after:?}"
    );
    assert_eq!(
        (after.frames_failed, after.source_failures),
        (0, 0),
        "пропуск — не сбой"
    );

    assert_eq!(
        screen.scopes.counters(),
        DeliveryCounters {
            show_delivered: 1,
            hide_delivered: 1,
            ..DeliveryCounters::default()
        }
    );
    bench.finish();
}

#[test]
fn a_screen_show_vectorscope_oklab_intent_makes_the_gpu_use_oklab_axes() {
    let scenes = [reddish(), bluish()];
    let Some(mut bench) = ViewfinderBench::start_with_scenes(&config(), &scenes) else {
        return;
    };
    let oklab = request(ScopeKind::Vectorscope, VectorscopeAxes::OklabAb);
    let ycbcr = request(ScopeKind::Vectorscope, VectorscopeAxes::YCbCrBt709);
    let mut screen = ScreenToRender::new();

    install_scope_screen(&mut bench, oklab.image_size);
    screen.dispatch(ScopeIntent::Show(oklab));
    assert_eq!(screen.deliver_to(&mut bench), vec![ViewfinderCommandOutcome::Applied]);

    for frame in 1..=scenes.len() {
        bench.show_next_frame();
        let gpu = gpu_counters(&bench);
        let (expected_oklab, _, _) = reference(&bench, &oklab);
        let (expected_ycbcr, _, _) = reference(&bench, &ycbcr);
        assert_eq!(gpu, expected_oklab, "кадр {frame}: GPU считает по осям OKLab");
        assert_ne!(
            expected_oklab, expected_ycbcr,
            "кадр {frame}: оси дают разные счётчики, иначе тест не отличил бы их"
        );
        assert_ne!(gpu, expected_ycbcr, "кадр {frame}: это не Cb/Cr");
    }
    assert_eq!(scopes_of(&mut bench).requested, Some(oklab));
    bench.finish();
}

/// Кнопка «2×» (решение 325) тем же путём экрана: GPU считает вектороскоп с увеличением — счётчики равны эталону
/// увеличенного запроса и отличаются от обычного масштаба.
#[test]
fn a_screen_show_vectorscope_at_double_zoom_makes_the_gpu_magnify_the_centre() {
    let scenes = [reddish(), bluish()];
    let Some(mut bench) = ViewfinderBench::start_with_scenes(&config(), &scenes) else {
        return;
    };
    let normal = request(ScopeKind::Vectorscope, VectorscopeAxes::YCbCrBt709);
    let double = ScopeRequest {
        vectorscope_zoom: scope_contract::VectorscopeZoom::Double,
        cie_diagram: scope_contract::CieDiagram::default(),
        histogram_mode: scope_contract::HistogramMode::Rgb,
        ..normal
    };
    let mut screen = ScreenToRender::new();
    install_scope_screen(&mut bench, double.image_size);
    screen.dispatch(ScopeIntent::Show(double));
    assert_eq!(screen.deliver_to(&mut bench), vec![ViewfinderCommandOutcome::Applied]);
    for frame in 1..=scenes.len() {
        bench.show_next_frame();
        let gpu = gpu_counters(&bench);
        let (expected_double, _, _) = reference(&bench, &double);
        let (expected_normal, _, _) = reference(&bench, &normal);
        assert_eq!(gpu, expected_double, "кадр {frame}: GPU считает с увеличением ×2");
        assert_ne!(
            expected_double, expected_normal,
            "кадр {frame}: увеличение меняет счётчики"
        );
        assert_eq!(
            gpu.shape(),
            expected_normal.shape(),
            "кадр {frame}: сетка та же — ресурсы не пересоздаются"
        );
    }
    assert_eq!(scopes_of(&mut bench).requested, Some(double));
    bench.finish();
}

/// Страница LUMA WAVEFORM (решение 321) тем же путём экрана: GPU считает одну плоскость яркости, счётчики — эталон
/// показанного кадра, картинка прибора — эталон показа и белый след; это не RGB waveform (три плоскости).
#[test]
fn a_screen_show_luma_waveform_intent_draws_the_one_plane_luma_trace_of_the_shown_frame() {
    let scenes = [reddish(), bluish()];
    let Some(mut bench) = ViewfinderBench::start_with_scenes(&config(), &scenes) else {
        return;
    };
    let luma = request(ScopeKind::LumaWaveform, VectorscopeAxes::YCbCrBt709);
    let rgb = request(ScopeKind::Waveform, VectorscopeAxes::YCbCrBt709);
    let mut screen = ScreenToRender::new();

    install_scope_screen(&mut bench, luma.image_size);
    screen.dispatch(ScopeIntent::Show(luma));
    assert_eq!(screen.deliver_to(&mut bench), vec![ViewfinderCommandOutcome::Applied]);

    for frame in 1..=scenes.len() {
        bench.show_next_frame();
        let gpu = gpu_counters(&bench);
        let (expected, rule, samples) = reference(&bench, &luma);
        let (expected_rgb, _, _) = reference(&bench, &rgb);
        assert_eq!(gpu, expected, "кадр {frame}: GPU считает яркость показанного кадра");
        assert_eq!(gpu.shape().planes(), 1, "кадр {frame}: одна плоскость");
        assert_ne!(
            gpu.shape(),
            expected_rgb.shape(),
            "кадр {frame}: это не три плоскости RGB"
        );
        let picture = bench
            .backend
            .read_scope_picture()
            .expect("чтение картинки прибора")
            .expect("картинка есть");
        let model = small_model();
        let reference_picture =
            render(&expected, &rule, &model.render, luma.image_size, samples).expect("эталон показа");
        let worst = worst_premultiplied_difference(&picture.texels, reference_picture.pixels());
        assert!(
            worst <= PICTURE_TOLERANCE,
            "кадр {frame}: картинка разошлась с эталоном на {worst}"
        );
        // След белый: у самых ярких пикселей следа цвет — цвет следа яркости из настроек.
        let brightest = picture
            .texels
            .iter()
            .copied()
            .max_by(|a, b| a[3].total_cmp(&b[3]))
            .expect("пиксели есть");
        assert!(brightest[3] > 0.5, "кадр {frame}: след есть");
        for channel in 0..3 {
            assert!(
                (brightest[channel] - model.render.luma_trace_color[channel]).abs() < 2e-2,
                "кадр {frame}: след белый, {brightest:?}"
            );
        }
    }
    assert_eq!(scopes_of(&mut bench).requested, Some(luma));
    bench.finish();
}

/// Страница HISTOGRAM (сессия 14s) тем же путём экрана во всех трёх режимах: GPU считает уровни показанного кадра в
/// сетке «уровни × одна строка» (три плоскости у RGB и PARADE, одна у Y), счётчики — эталон показанного кадра, картинка
/// прибора — эталон показа; серый сюжет даёт столбик на ОДНОМ уровне во всех плоскостях.
#[test]
fn a_screen_show_histogram_intent_draws_the_bars_of_the_shown_frame_in_every_mode() {
    let scenes = [neutral_gray(), reddish(), bluish()];
    let Some(mut bench) = ViewfinderBench::start_with_scenes(&config(), &scenes) else {
        return;
    };
    let mut screen = ScreenToRender::new();
    let histogram = |mode| ScopeRequest {
        histogram_mode: mode,
        ..request(ScopeKind::Histogram, VectorscopeAxes::YCbCrBt709)
    };
    install_scope_screen(&mut bench, histogram(HistogramMode::Rgb).image_size);
    // Каждый режим — свой запрос на ТОМ ЖЕ показанном кадре: рендер сам считает ближайший кадр заново (смена запроса не
    // ждёт каденции), поэтому тест переключает режим перед каждым кадром сюжета.
    for (frame, mode) in [HistogramMode::Rgb, HistogramMode::Parade, HistogramMode::Luma]
        .into_iter()
        .enumerate()
    {
        let scope = histogram(mode);
        screen.dispatch(ScopeIntent::Show(scope));
        assert_eq!(screen.deliver_to(&mut bench), vec![ViewfinderCommandOutcome::Applied]);
        bench.show_next_frame();
        assert_eq!(
            scopes_of(&mut bench).requested,
            Some(scope),
            "{mode:?}: запрос дошёл до рендера"
        );
        let gpu = gpu_counters(&bench);
        let (expected, rule, samples) = reference(&bench, &scope);
        assert_eq!(
            gpu, expected,
            "{mode:?}: счётчики GPU разошлись с эталоном показанного кадра"
        );
        let planes = if mode == HistogramMode::Luma { 1 } else { 3 };
        assert_eq!(
            (gpu.shape().columns(), gpu.shape().rows(), gpu.shape().planes()),
            (small_model().histogram_bins, 1, planes),
            "{mode:?}: форма сетки гистограммы"
        );
        // Каждый отсчёт кадра — одно попадание в каждую плоскость.
        assert_eq!(gpu.total(), u64::from(planes) * samples.total(), "{mode:?}");
        if frame == 0 {
            // Серый сюжет: во всех плоскостях заняты одни и те же уровни (столбец — уровень кода, а не положение).
            let occupied = |plane: u32| -> Vec<u32> {
                (0..gpu.shape().columns())
                    .filter(|&bin| gpu.count(plane, 0, bin).unwrap_or(0) > 0)
                    .collect()
            };
            assert!(!occupied(0).is_empty());
            assert_eq!(occupied(0), occupied(1), "серый: красная и зелёная плоскости совпали");
            assert_eq!(occupied(0), occupied(2), "серый: красная и синяя плоскости совпали");
        }
        let picture = bench
            .backend
            .read_scope_picture()
            .expect("чтение картинки прибора")
            .expect("картинка есть");
        let model = small_model();
        let reference_picture =
            render(&expected, &rule, &model.render, scope.image_size, samples).expect("эталон показа");
        let worst = worst_premultiplied_difference(&picture.texels, reference_picture.pixels());
        assert!(
            worst <= PICTURE_TOLERANCE,
            "{mode:?}: картинка разошлась с эталоном на {worst}"
        );
        let lit = picture.texels.iter().filter(|pixel| pixel[3] > 0.5).count();
        assert!(lit > 50, "{mode:?}: столбики нарисованы ({lit} ярких пикселей)");
    }
    bench.finish();
}

/// Страница CIE (сессия 14s) тем же путём экрана в обеих шкалах: GPU считает цветность показанного кадра в квадратной
/// сетке, счётчики и картинка — эталон; серый сюжет ложится в ячейку точки белого D65 своей шкалы; шкалы дают разные
/// счётчики.
#[test]
fn a_screen_show_cie_intent_plots_the_shown_frame_chromaticity_in_both_diagrams() {
    let scenes = [neutral_gray(), reddish(), bluish()];
    let Some(mut bench) = ViewfinderBench::start_with_scenes(&config(), &scenes) else {
        return;
    };
    let mut screen = ScreenToRender::new();
    let cie = |diagram| ScopeRequest {
        cie_diagram: diagram,
        image_size: ScopeImageSize::try_new(128, 128).expect("размер"),
        ..request(ScopeKind::Cie, VectorscopeAxes::YCbCrBt709)
    };
    install_scope_screen(&mut bench, cie(scope_contract::CieDiagram::Xy1931).image_size);
    let mut seen = Vec::new();
    for (frame, diagram) in [
        scope_contract::CieDiagram::Xy1931,
        scope_contract::CieDiagram::UvPrime1976,
    ]
    .into_iter()
    .enumerate()
    {
        let scope = cie(diagram);
        screen.dispatch(ScopeIntent::Show(scope));
        assert_eq!(screen.deliver_to(&mut bench), vec![ViewfinderCommandOutcome::Applied]);
        bench.show_next_frame();
        assert_eq!(
            scopes_of(&mut bench).requested,
            Some(scope),
            "{diagram:?}: запрос дошёл до рендера"
        );
        let gpu = gpu_counters(&bench);
        let (expected, rule, samples) = reference(&bench, &scope);
        assert_eq!(
            gpu, expected,
            "{diagram:?}: счётчики GPU разошлись с эталоном показанного кадра"
        );
        assert_eq!(
            (gpu.shape().columns(), gpu.shape().rows(), gpu.shape().planes()),
            (512, 512, 1)
        );
        assert!(
            gpu.total() > 0 && gpu.total() <= samples.total(),
            "{diagram:?}: попадания есть"
        );
        if frame == 0 {
            // Серый сюжет: все попадания — в одну-две соседние ячейки вокруг D65 (код серого по ступеням чуть разный).
            let window = match diagram {
                scope_contract::CieDiagram::Xy1931 => scope_model::ChartWindow::XY_DEFAULT,
                scope_contract::CieDiagram::UvPrime1976 => scope_model::ChartWindow::UV_DEFAULT,
            };
            let projection = scope_model::ChartProjection::new(diagram, window);
            let d65 = projection.picture_point(isp_model::color::chromaticity::Chromaticity::D65);
            let (column, row) = ((d65[0] * 512.0) as i64, (d65[1] * 512.0) as i64);
            let near: u64 = (-2..=2)
                .flat_map(|dy| (-2..=2).map(move |dx| (dx, dy)))
                .map(|(dx, dy)| u64::from(gpu.count(0, (row + dy) as u32, (column + dx) as u32).unwrap_or(0)))
                .sum();
            assert_eq!(near, gpu.total(), "серый кадр целиком лежит у точки белого D65");
        }
        let picture = bench
            .backend
            .read_scope_picture()
            .expect("чтение картинки прибора")
            .expect("картинка есть");
        let reference_picture =
            render(&expected, &rule, &small_model().render, scope.image_size, samples).expect("эталон показа");
        let worst = worst_premultiplied_difference(&picture.texels, reference_picture.pixels());
        assert!(
            worst <= PICTURE_TOLERANCE,
            "{diagram:?}: картинка разошлась с эталоном на {worst}"
        );
        let lit = picture.texels.iter().filter(|pixel| pixel[3] > 0.5).count();
        assert!(lit > 20, "{diagram:?}: след и сетка нарисованы ({lit} ярких пикселей)");
        seen.push(gpu);
    }
    assert_ne!(seen[0], seen[1], "шкалы xy и u′v′ кладут цвет в разные ячейки");
    bench.finish();
}

/// Смена прибора меняет размер картинки (waveform — вся зона, вектороскоп — квадрат): пока новая картинка не
/// записана, UI-кадр рисует элемент НОВОГО размера, а у рендера лежит картинка СТАРОГО. Растянутая картинка —
/// овал вместо круга, поэтому такой кадр элемент не рисует (`live_images_missing`), а когда картинка нового размера
/// готова — рисует снова.
#[test]
fn a_scope_change_to_another_picture_size_never_draws_a_stretched_stale_picture() {
    let scenes = vec![neutral_gray(); 8];
    let Some(mut bench) = ViewfinderBench::start_with_scenes(&config(), &scenes) else {
        return;
    };
    let waveform = request(ScopeKind::Waveform, VectorscopeAxes::YCbCrBt709);
    let square = ScopeRequest {
        image_size: ScopeImageSize::try_new(96, 96).expect("размер"),
        ..request(ScopeKind::Vectorscope, VectorscopeAxes::YCbCrBt709)
    };
    let mut screen = ScreenToRender::new();
    let screen_size = install_scope_screen(&mut bench, waveform.image_size);
    screen.dispatch(ScopeIntent::Show(waveform));
    assert_eq!(screen.deliver_to(&mut bench), vec![ViewfinderCommandOutcome::Applied]);

    // Прибор показан: каждый кадр UI рисует картинку.
    bench.show_next_frame();
    bench.show_next_frame();
    let steady = ui_report(&mut bench);
    assert!(steady.live_images_drawn >= 1, "картинка рисуется: {steady:?}");
    assert_eq!(steady.live_images_missing, 0);

    // Экран уже рисует квадрат, а рендер ещё держит прямоугольник 160×96: кадр картинку не рисует.
    *screen_size.lock().expect("замок") = square.image_size;
    bench.show_next_frame();
    let mismatched = ui_report(&mut bench);
    assert_eq!(
        mismatched.live_images_drawn, steady.live_images_drawn,
        "растянутая картинка не нарисована"
    );
    assert!(
        mismatched.live_images_missing > steady.live_images_missing,
        "{mismatched:?}"
    );
    assert_eq!(
        (mismatched.frames_failed, mismatched.source_failures),
        (0, 0),
        "пропуск — не сбой"
    );

    // Рендер получает запрос квадрата, готовит картинку нового размера и записывает её: рисуется снова.
    screen.dispatch(ScopeIntent::Show(square));
    assert_eq!(screen.deliver_to(&mut bench), vec![ViewfinderCommandOutcome::Applied]);
    for _ in 0..3 {
        bench.show_next_frame();
    }
    let recovered = ui_report(&mut bench);
    assert!(
        recovered.live_images_drawn > mismatched.live_images_drawn,
        "картинка нового размера нарисована: {recovered:?}"
    );
    let picture = bench
        .backend
        .read_scope_picture()
        .expect("чтение картинки прибора")
        .expect("картинка есть");
    assert_eq!(
        (picture.size.width(), picture.size.height()),
        (96, 96),
        "рисуется картинка нового, квадратного размера"
    );
    bench.finish();
}
