//! Цена быстрых путей стекла (сессия 16a): боевой шейдер против замороженного эталона, по очереди (A/B/A/B…),
//! на сценах «типичный кадр камеры» (8 стёкол) и «панель на пол-экрана». Плюс расчёт на CPU: какая доля пикселей
//! панели идёт по каждому быстрому пути. Порогов по времени нет (цена зависит от GPU), печатается таблица;
//! бенч (помечен `ignore`, запускается вручную: `--release -- --ignored --nocapture`) проверяет только то, что сравниваемое измерено, а доли быстрых путей на панели действительно высоки.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod saturation_support;
mod support;

use std::time::Duration;

use gpu_vulkan::backdrop::BackdropSource;
use gpu_vulkan::color_target::{ColorTarget, ColorTargetDesc, ColorTargetPreviousUse, ContentOnPrepare};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::raster::{AttachmentLoad, RasterPass, record_between_color_passes};
use gpu_vulkan::saturation::{
    CancelToken, FrameRecording, SaturationBench, SaturationConfig, SaturationWorkload, SlotId, Throughput,
};
use gpu_vulkan::swapchain::FramesInFlight;
use gpu_vulkan::test_pattern::SurfaceTransform;
use saturation_support::{BACKDROP_EXTENT, PANEL_CENTER, VIEWFINDER, panel_draw_list, panel_group, typical_draw_list};
use support::{assert_no_validation_errors, gpu_context_or_skip};
use ui_draw::draw_list::DrawList;
use ui_draw::geometry::{PxPoint, PxRect, PxSize};
use ui_draw::shape::signed_distance;
use ui_render_vulkan::saturation::{DEFAULT_TARGET_EXTENT, DEFAULT_TARGET_FORMAT};
use ui_render_vulkan::testing::{BackdropPattern, SyntheticBackdrop};
use ui_render_vulkan::testing_reference::{frozen_reference_renderer, whole_glass_quad_renderer};
use ui_render_vulkan::{BackdropInput, BackdropPlacement, LiveImages, UiFrame, UiRenderer, UiRendererConfig};

/// Слотов у стенда (как у боевых тестов стенда).
const SLOTS: usize = 2;
/// Сколько раз повторяется пара «эталон, боевой» (дрейф частоты GPU усредняется порядком A/B/A/B…).
const ROUNDS: usize = 4;

/// Какими шейдерами рисует работа стенда.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShaderChoice {
    FrozenReference,
    /// Боевые шейдеры, но квад стекла целиком (как до G2).
    ProductWholeQuad,
    Product,
}

/// Работа стенда: очистка цели + проход UI одного и того же списка (пирамида фона строится один раз и
/// переиспользуется, как на тиках 120 Гц); отличается только набор шейдеров стекла и фигур.
struct UiPassWork {
    renderer: UiRenderer,
    targets: Vec<ColorTarget>,
    target_used: Vec<bool>,
    draw_list: DrawList,
    source: BackdropSource,
    _backdrop_image: SyntheticBackdrop,
    context: GpuContext,
}

impl UiPassWork {
    fn new(context: &GpuContext, choice: ShaderChoice, draw_list: DrawList) -> Self {
        let format = DEFAULT_TARGET_FORMAT;
        let config = UiRendererConfig::new(format.vk_format(), SLOTS);
        let renderer = match choice {
            ShaderChoice::FrozenReference => frozen_reference_renderer(context, &config).unwrap(),
            ShaderChoice::ProductWholeQuad => whole_glass_quad_renderer(context, &config).unwrap(),
            ShaderChoice::Product => UiRenderer::new(context, &config).unwrap(),
        };
        let targets = (0..SLOTS)
            .map(|_| {
                ColorTarget::new(
                    context,
                    &ColorTargetDesc {
                        extent: DEFAULT_TARGET_EXTENT,
                        format,
                        label: "glass-fast-paths-bench",
                    },
                )
                .unwrap()
            })
            .collect();
        let image = SyntheticBackdrop::new(context, BACKDROP_EXTENT, BackdropPattern::Photo).unwrap();
        let source = image.source(1);
        Self {
            renderer,
            targets,
            target_used: vec![false; SLOTS],
            draw_list,
            source,
            _backdrop_image: image,
            context: context.clone(),
        }
    }
}

impl SaturationWorkload for UiPassWork {
    fn record_frame(&mut self, recording: FrameRecording) -> Result<(), GpuError> {
        let slot = recording.slot().index();
        let command_buffer = recording.command_buffer();
        let target = &self.targets[slot];
        let previous_use = if self.target_used[slot] {
            ColorTargetPreviousUse::Rendered
        } else {
            ColorTargetPreviousUse::NotUsedYet
        };
        target.record_prepare_for_render(command_buffer, previous_use, ContentOnPrepare::Discard)?;
        RasterPass::begin(
            &self.context,
            command_buffer,
            target.attachment(),
            AttachmentLoad::Clear([0.0, 0.0, 0.0, 1.0]),
        )?
        .end();
        record_between_color_passes(&self.context, command_buffer);
        let frame = UiFrame {
            draw_list: &self.draw_list,
            logical_size: PxSize::new(DEFAULT_TARGET_EXTENT.width as f32, DEFAULT_TARGET_EXTENT.height as f32),
            surface_transform: SurfaceTransform::Identity,
            backdrop: BackdropInput::Live {
                source: &self.source,
                placement: BackdropPlacement {
                    destination: PxRect::new(
                        PxPoint::new(VIEWFINDER.x as f32, VIEWFINDER.y as f32),
                        PxSize::new(VIEWFINDER.width as f32, VIEWFINDER.height as f32),
                    ),
                },
            },
            live_images: LiveImages::none(),
        };
        self.renderer
            .record_frame(command_buffer, slot, target.attachment(), &frame)
            .map_err(|error| GpuError::InvalidState {
                what: format!("рендер UI в бенче быстрых путей: {error}"),
            })?;
        self.target_used[slot] = true;
        Ok(())
    }

    fn release_slot_after_fence(&mut self, slot: SlotId) {
        self.renderer
            .release_slot_after_fence(slot.index())
            .expect("слот рендера освобождается после fence");
    }
}

/// Миллисекунды на кадр при насыщении для набора шейдеров и списка.
fn measure_frame_cost_ms(context: &GpuContext, choice: ShaderChoice, list: &DrawList) -> f64 {
    let mut work = UiPassWork::new(context, choice, list.clone());
    let config = SaturationConfig {
        frames_in_flight: FramesInFlight::new(SLOTS).unwrap(),
        warmup: Duration::from_millis(300),
        measure: Duration::from_millis(1000),
        timeline_step: Duration::from_millis(500),
        fence_timeout: Duration::from_secs(20),
    };
    let mut bench = SaturationBench::new(context, config).unwrap();
    let report = bench.run(&mut work, &CancelToken::new()).unwrap();
    match report.throughput {
        Throughput::Measured { frame_cost, .. } => frame_cost.as_secs_f64() * 1e3,
        Throughput::NoFramesCompleted => panic!("в окне замера нет кадров: {report:?}"),
    }
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

/// Печатает «эталон против боевого» по сцене; возвращает медианы (эталон, боевой), мс.
fn print_ab(context: &GpuContext, name: &str, list: &DrawList, before_choice: ShaderChoice) -> (f64, f64) {
    let (mut reference, mut product) = (Vec::new(), Vec::new());
    for _ in 0..ROUNDS {
        reference.push(measure_frame_cost_ms(context, before_choice, list));
        product.push(measure_frame_cost_ms(context, ShaderChoice::Product, list));
    }
    println!("{name}: эталон (до) мс/кадр {reference:.3?}; боевой (после) мс/кадр {product:.3?}");
    let (before, after) = (median(&mut reference), median(&mut product));
    println!(
        "{name}: медиана до {before:.3} мс, после {after:.3} мс, выигрыш {:.1} %",
        (1.0 - after / before) * 100.0
    );
    (before, after)
}

#[test]
#[ignore = "печатающий бенч на ~20 с: запуск вручную (--ignored --nocapture), порогов нет"]
fn glass_fast_paths_bench_prints() {
    let Some(context) = gpu_context_or_skip() else { return };
    println!("кадр 1080x2412, слотов {SLOTS}, пары A/B повторены {ROUNDS} раз (порядок: эталон, боевой, эталон, ...)");
    let (typical_before, typical_after) = print_ab(
        &context,
        "8 стёкол (типичный кадр), G1+G2 против эталона",
        &typical_draw_list(),
        ShaderChoice::FrozenReference,
    );
    let (panel_before, panel_after) = print_ab(
        &context,
        "панель на пол-экрана, G1+G2 против эталона",
        &panel_draw_list(),
        ShaderChoice::FrozenReference,
    );
    // Вклад одного G2 (узкая область при тех же боевых шейдерах): квад целиком против scissor.
    print_ab(
        &context,
        "8 стёкол, только G2",
        &typical_draw_list(),
        ShaderChoice::ProductWholeQuad,
    );
    print_ab(
        &context,
        "панель, только G2",
        &panel_draw_list(),
        ShaderChoice::ProductWholeQuad,
    );
    // Цена измерена и конечна; направление (после <= до) на ПК не гарантируем жёстко: RADV — не Adreno, шум возможен.
    for value in [typical_before, typical_after, panel_before, panel_after] {
        assert!(value.is_finite() && value > 0.0, "цена кадра не измерена: {value}");
    }
    assert_no_validation_errors(&context);
}

/// Доли пикселей панели по быстрым путям (расчёт на CPU по тем же формулам, что в шейдере: расстояние до края
/// из эталона `ui_draw::shape`, зона линзы и ободок из параметров стиля продукта).
#[test]
fn panel_fast_path_shares_are_high() {
    let group = panel_group();
    let geometry = group.members[0].geometry;
    let params = group.params;
    let bounds = group.adaptation_rect_px().unwrap();
    let (mut in_quad, mut covered, mut no_refraction, mut opaque, mut no_rim) = (0_u64, 0_u64, 0_u64, 0_u64, 0_u64);
    // Пиксели квада, где шейдер даёт ровно ноль (нет покрытия и тень нулевая), но всё равно считает два поля формы.
    let mut wasted = 0_u64;
    let first_row = bounds.origin.y.max(0.0) as u32;
    let last_row = (bounds.max_y() as u32).min(DEFAULT_TARGET_EXTENT.height);
    let first_column = bounds.origin.x.max(0.0) as u32;
    let last_column = (bounds.max_x() as u32).min(DEFAULT_TARGET_EXTENT.width);
    for y in first_row..last_row {
        for x in first_column..last_column {
            in_quad += 1;
            let local = PxPoint::new(x as f32 + 0.5 - PANEL_CENTER.x, y as f32 + 0.5 - PANEL_CENTER.y);
            let distance = signed_distance(&geometry, local);
            // Покрытие шейдера положительно, пока расстояние меньше половины ширины сглаживания (0,5 px).
            if distance >= 0.5 {
                let shadow_distance =
                    signed_distance(&geometry, PxPoint::new(local.x, local.y - params.shadow.offset_px));
                if shadow_distance >= params.shadow.radius_px {
                    wasted += 1;
                }
                continue;
            }
            covered += 1;
            let depth = -distance;
            if depth >= params.lens_zone_px {
                no_refraction += 1;
            }
            if distance <= -0.5 {
                opaque += 1;
            }
            if depth >= params.specular.rim_width_px {
                no_rim += 1;
            }
        }
    }
    let share = |part: u64, whole: u64| part as f64 / whole as f64 * 100.0;
    println!(
        "панель {}x{}: в квадре группы {in_quad} пикселей, с покрытием {covered} ({:.1} % квадра)",
        saturation_support::PANEL_SIZE.width,
        saturation_support::PANEL_SIZE.height,
        share(covered, in_quad)
    );
    println!(
        "  от покрытых пикселей: без преломления (одна выборка вместо трёх) {:.1} %; непрозрачные (без тени) {:.1} %; \
         вне ободка (без pow блика) {:.1} %",
        share(no_refraction, covered),
        share(opaque, covered),
        share(no_rim, covered)
    );
    println!(
        "  пустые пиксели квада (нет стекла, тени тоже ноль, но оба поля формы считаются): {wasted} = {:.1} % квада; \
         квад раздут свечением нажатия (радиус {} px) даже когда нажатия нет",
        share(wasted, in_quad),
        params.press.glow_radius_px
    );
    println!(
        "  размытие: радиус {:.3} px; целый уровень пирамиды только при радиусе 1,5 * 2^k px (здесь дробный: 0 %)",
        params.blur_radius_px
    );
    assert!(
        covered > 0 && covered < in_quad,
        "панель должна занимать часть своего квада"
    );
    assert!(
        share(no_refraction, covered) > 70.0,
        "большая часть панели глубже зоны линзы"
    );
    assert!(share(opaque, covered) > 95.0, "почти вся панель непрозрачна для тени");
    assert!(share(no_rim, covered) > 90.0, "почти вся панель вне ободка блика");
}

/// Пересечение прямоугольника с экраном 1080 × 2412, в пикселях.
fn pixels_on_screen(rect: &PxRect) -> u64 {
    let width = (rect.max_x().min(DEFAULT_TARGET_EXTENT.width as f32) - rect.origin.x.max(0.0)).max(0.0);
    let height = (rect.max_y().min(DEFAULT_TARGET_EXTENT.height as f32) - rect.origin.y.max(0.0)).max(0.0);
    (width.ceil() as u64) * (height.ceil() as u64)
}

/// Сколько фрагментов отдаёт панели растеризатор до и после узкой области (scissor), и по какой области
/// фактически берётся «средняя яркость под группой» (это прямоугольник адаптации, он НЕ менялся).
#[test]
fn panel_fragment_counts_and_adaptation_area() {
    let group = panel_group();
    let quad = group.adaptation_rect_px().unwrap();
    let draw = group.draw_rect_px().unwrap();
    let (before, after) = (pixels_on_screen(&quad), pixels_on_screen(&draw));
    println!(
        "панель: квад ДО {before} фрагментов (прямоугольник {quad:?}); область вклада ПОСЛЕ {after} ({:.1} % от прежнего); \
         сама панель {} px",
        after as f64 / before as f64 * 100.0,
        (saturation_support::PANEL_SIZE.width * saturation_support::PANEL_SIZE.height) as u64
    );
    // Точки средней яркости: центр и четверти прямоугольника адаптации (как в шейдере и в `mean_luma_under`).
    let (center, quarter) = (
        quad.center(),
        PxPoint::new(quad.size.width * 0.25, quad.size.height * 0.25),
    );
    println!(
        "  средняя яркость берётся в точках: центр ({:.0}, {:.0}), четверти ±({:.0}, {:.0}) -> x от {:.0} до {:.0}, y от {:.0} до {:.0}; \
         экран 1080 x 2412, панель x {:.0}..{:.0}",
        center.x,
        center.y,
        quarter.x,
        quarter.y,
        center.x - quarter.x,
        center.x + quarter.x,
        center.y - quarter.y,
        center.y + quarter.y,
        PANEL_CENTER.x - saturation_support::PANEL_SIZE.width / 2.0,
        PANEL_CENTER.x + saturation_support::PANEL_SIZE.width / 2.0
    );
    assert!(
        after * 10 < before * 6,
        "область вклада должна быть заметно уже прежнего квада: {after} из {before}"
    );
    assert!(after > 1_000_000, "панель 540 x 1950 сама даёт около миллиона пикселей");
}
