//! GPU-тесты места видоискателя и UI поверх чёрного кадра (сессия 12) на offscreen-цели, которую можно
//! ПРОЧИТАТЬ (swapchain headless-стенда читать нельзя). Настоящие `CameraPipelines`, компоновщик и рендер
//! UI на реальном драйвере с полной validation; цель кадра строится через `FrameTarget`.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ash::vk;
use gpu_vulkan::context::{GpuContext, GpuContextConfig};
use gpu_vulkan::device::DeviceProfile;
use gpu_vulkan::offscreen::{OffscreenTarget, Rgba8Image};
use gpu_vulkan::sampled_texture::{SampledTexture, TextureDesc, TextureFormat};
use gpu_vulkan::swapchain::{DisplayColorSpace, DisplayTargetInfo};
use gpu_vulkan::test_pattern::SurfaceTransform;
use gpu_vulkan::validation::ValidationMode;
use isp_vulkan::viewfinder::ViewfinderIspConfig;
use shell_contract::clock::MonotonicNanos;
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{DrawItem, DrawList, ShapeItem};
use ui_draw::frame_source::{UiFrameError, UiFrameRequest, UiFrameSource};
use ui_draw::light::LightDirection;
use ui_draw::shape::ShapeGeometry;
use ui_draw::transform::Affine2;

use super::{CameraPipelines, FrameTarget, ScreenContent};
use crate::config::UiOverlayConfig;
use crate::ui_overlay::{UiOverlayCommand, UiOverlayOutcome};
use crate::viewfinder::{EffectivePlacement, NormalizedScreenRect};
use crate::vulkan_backend::camera::SubmissionSerial;
use crate::vulkan_backend::camera::ui_state::{UiOverlayFrame, UiOverlayState};

const SKIP_ENV: &str = "FASTICAMERA_SKIP_GPU_TESTS";
/// Цель 100×200 (портрет, без поворота).
const TARGET_WIDTH: u32 = 100;
const TARGET_HEIGHT: u32 = 200;
const BLACK: [u8; 4] = [0, 0, 0, 255];

fn context_or_skip() -> Option<GpuContext> {
    match GpuContext::new(&GpuContextConfig::new(DeviceProfile::Headless, ValidationMode::FULL)) {
        Ok(context) => Some(context),
        Err(error) => {
            if std::env::var(SKIP_ENV).as_deref() == Ok("1") {
                eprintln!("ПРОПУСК GPU-теста: {error} ({SKIP_ENV}=1)");
                None
            } else {
                panic!("GPU недоступен: {error}. Установите {SKIP_ENV}=1, чтобы пропустить GPU-тесты осознанно");
            }
        }
    }
}

/// Стенд: настоящие конвейеры показа камеры + читаемая цель.
struct Stand {
    pipelines: CameraPipelines,
    target: OffscreenTarget,
}

impl Stand {
    fn new(context: &GpuContext) -> Self {
        let info = DisplayTargetInfo {
            format: OffscreenTarget::FORMAT,
            color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR,
            display_color_space: DisplayColorSpace::Srgb,
            extent: vk::Extent2D {
                width: TARGET_WIDTH,
                height: TARGET_HEIGHT,
            },
            logical_extent: vk::Extent2D {
                width: TARGET_WIDTH,
                height: TARGET_HEIGHT,
            },
            transform: SurfaceTransform::Identity,
            image_count: 1,
            composite_alpha: vk::CompositeAlphaFlagsKHR::OPAQUE,
            present_mode: vk::PresentModeKHR::FIFO,
            frames_in_flight: 1,
            recreation_count: 0,
            display_timing_supported: false,
        };
        Self {
            pipelines: CameraPipelines::new(
                context,
                &info,
                &ViewfinderIspConfig::default(),
                &UiOverlayConfig::default(),
            )
            .expect("конвейеры показа камеры"),
            target: OffscreenTarget::new(context, TARGET_WIDTH, TARGET_HEIGHT).expect("offscreen-цель"),
        }
    }

    /// Один кадр: запись в цель замыканием, затем подтверждение fence слота и чтение пикселей.
    fn frame(
        &mut self,
        record: impl FnOnce(&mut CameraPipelines, &FrameTarget) -> Result<(), gpu_vulkan::error::GpuError>,
    ) -> Rgba8Image {
        let pipelines = &mut self.pipelines;
        self.target
            .render(|command_buffer, pattern_target| {
                record(
                    pipelines,
                    &FrameTarget {
                        command_buffer,
                        pattern_target,
                        format: OffscreenTarget::FORMAT,
                        display_color_space: DisplayColorSpace::Srgb,
                        transform: SurfaceTransform::Identity,
                        ring_slot: 0,
                    },
                )
            })
            .expect("кадр записан и выполнен");
        // Кадр выполнен (блокирующая отправка): слот снова свободен.
        let collected = self.pipelines.collect_slot(0);
        assert!(collected.compositor.is_ok(), "{:?}", collected.compositor);
        assert!(collected.ui.is_ok(), "{:?}", collected.ui);
        self.target.readback().expect("чтение цели")
    }
}

/// Текстура 8×8 (или 16×8) из четырёх цветных четвертей: красная, зелёная / синяя, белая.
fn quadrant_texture(context: &GpuContext, width: u32, height: u32) -> SampledTexture {
    let mut texels = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let color: [u8; 4] = match (x < width / 2, y < height / 2) {
                (true, true) => [255, 0, 0, 255],
                (false, true) => [0, 255, 0, 255],
                (true, false) => [0, 0, 255, 255],
                (false, false) => [255, 255, 255, 255],
            };
            texels.extend_from_slice(&color);
        }
    }
    SampledTexture::upload(
        context,
        &TextureDesc {
            extent: vk::Extent2D { width, height },
            format: TextureFormat::Rgba8Unorm,
            label: "четверти",
        },
        &texels,
    )
    .expect("текстура загружена")
}

fn rect(left: f32, top: f32, width: f32, height: f32) -> EffectivePlacement {
    EffectivePlacement::ScreenRect(NormalizedScreenRect::new(left, top, width, height).expect("прямоугольник"))
}

fn pixel(image: &Rgba8Image, x: u32, y: u32) -> [u8; 4] {
    image.pixel(x, y).expect("пиксель внутри цели")
}

/// Цвет близок к ожидаемому (линейная выборка у границ четвертей даёт отклонение).
fn assert_color(image: &Rgba8Image, x: u32, y: u32, expected: [u8; 3]) {
    let actual = pixel(image, x, y);
    for channel in 0..3 {
        assert!(
            actual[channel].abs_diff(expected[channel]) <= 3,
            "пиксель ({x},{y}) = {actual:?}, ждали {expected:?}"
        );
    }
}

#[test]
fn viewfinder_is_drawn_exactly_in_the_requested_rect_and_everything_else_is_black() {
    let Some(context) = context_or_skip() else { return };
    let mut stand = Stand::new(&context);
    let texture = quadrant_texture(&context, 8, 8);
    // Доли 0.1/0.5/0.8/0.4 на цели 100×200 = пиксели 10..90 по x и 100..180 по y: квадрат 80×80,
    // изображение 1:1 заполняет его целиком, без полос.
    let image = stand.frame(|pipelines, target| {
        pipelines
            .compose_view(target, texture.sampled_view(), rect(0.1, 0.5, 0.8, 0.4))
            .map(|_| ())
    });
    // Четверти на своих местах (проверяет и то, что изображение не перевёрнуто и не растянуто).
    assert_color(&image, 15, 105, [255, 0, 0]);
    assert_color(&image, 85, 105, [0, 255, 0]);
    assert_color(&image, 15, 175, [0, 0, 255]);
    assert_color(&image, 85, 175, [255, 255, 255]);
    // Снаружи — чёрный: слева/справа, выше и ниже прямоугольника, в том числе в верхней половине экрана.
    for (x, y) in [(5, 140), (95, 140), (50, 95), (50, 185), (50, 10), (5, 5)] {
        assert_eq!(pixel(&image, x, y), BLACK, "({x},{y}) вне прямоугольника");
    }
    context.wait_idle().expect("простой GPU");
    assert_eq!(context.validation_report().errors, 0);
}

#[test]
fn image_with_other_proportions_is_fitted_not_stretched_and_the_rest_of_the_rect_stays_black() {
    let Some(context) = context_or_skip() else { return };
    let mut stand = Stand::new(&context);
    // Прямоугольник 80×80 (10..90, 100..180); изображение 2:1 → 80×40 по центру: y = 120..160.
    let wide = quadrant_texture(&context, 16, 8);
    let image = stand.frame(|pipelines, target| {
        pipelines
            .compose_view(target, wide.sampled_view(), rect(0.1, 0.5, 0.8, 0.4))
            .map(|_| ())
    });
    assert_color(&image, 15, 125, [255, 0, 0]);
    assert_color(&image, 85, 155, [255, 255, 255]);
    for (x, y) in [(50, 110), (50, 170), (50, 98), (50, 185)] {
        assert_eq!(pixel(&image, x, y), BLACK, "({x},{y}) в полосе над/под изображением");
    }
    // Относительно более высокое изображение (1:2) вписывается по высоте: 40×80 по центру, x = 30..70.
    let tall = quadrant_texture(&context, 8, 16);
    let image = stand.frame(|pipelines, target| {
        pipelines
            .compose_view(target, tall.sampled_view(), rect(0.1, 0.5, 0.8, 0.4))
            .map(|_| ())
    });
    assert_color(&image, 35, 105, [255, 0, 0]);
    assert_color(&image, 65, 175, [255, 255, 255]);
    for (x, y) in [(15, 140), (85, 140), (25, 140), (75, 140)] {
        assert_eq!(pixel(&image, x, y), BLACK, "({x},{y}) в полосе слева/справа");
    }
    context.wait_idle().expect("простой GPU");
    assert_eq!(context.validation_report().errors, 0);
}

#[test]
fn next_frame_with_a_new_rect_leaves_nothing_in_the_old_place() {
    let Some(context) = context_or_skip() else { return };
    let mut stand = Stand::new(&context);
    let texture = quadrant_texture(&context, 8, 8);
    stand.frame(|pipelines, target| {
        pipelines
            .compose_view(target, texture.sampled_view(), rect(0.0, 0.0, 1.0, 0.5))
            .map(|_| ())
    });
    // Прямоугольник сменился одной командой: следующий кадр целиком в новом месте, старого следа нет.
    let image = stand.frame(|pipelines, target| {
        pipelines
            .compose_view(target, texture.sampled_view(), rect(0.0, 0.5, 1.0, 0.5))
            .map(|_| ())
    });
    assert_eq!(pixel(&image, 50, 50), BLACK, "старое место очищено");
    assert_color(&image, 20, 120, [255, 0, 0]);
    assert_color(&image, 80, 180, [255, 255, 255]);
}

// ---- UI поверх чёрного кадра без камеры ----

/// Источник UI: большой белый круг в центре цели. `failing` — отвечает отказом.
struct DiscSource {
    calls: Arc<AtomicU64>,
    failing: bool,
}

impl UiFrameSource for DiscSource {
    fn name(&self) -> &'static str {
        "disc"
    }

    fn produce_frame(&mut self, request: &UiFrameRequest) -> Result<DrawList, UiFrameError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.failing {
            return Err(UiFrameError::SourceFailed {
                reason: "тестовый отказ".to_owned(),
            });
        }
        let mut list = DrawList::new(LightDirection::from_angle_degrees(45.0));
        list.items.push(DrawItem::Shape(ShapeItem {
            geometry: ShapeGeometry::Circle { diameter: 60.0 },
            transform: Affine2::translate(request.logical_size.width * 0.5, request.logical_size.height * 0.5),
            fill: DisplayColor::WHITE,
            stroke: None,
            shadow: None,
            opacity: Opacity::OPAQUE,
        }));
        Ok(list)
    }
}

fn install_disc(failing: bool) -> (UiOverlayState, Arc<AtomicU64>) {
    let calls = Arc::new(AtomicU64::new(0));
    let mut state = UiOverlayState::new(8);
    let outcome = state.apply(UiOverlayCommand::Install(Box::new(DiscSource {
        calls: Arc::clone(&calls),
        failing,
    })));
    assert_eq!(outcome, UiOverlayOutcome::Applied);
    (state, calls)
}

#[test]
fn ui_is_drawn_over_a_black_frame_when_there_is_no_camera_image() {
    let Some(context) = context_or_skip() else { return };
    let mut stand = Stand::new(&context);
    let (mut state, calls) = install_disc(false);
    stand.pipelines.prepare_ui_overlay().expect("рендер UI");
    let image = stand.frame(|pipelines, target| {
        // Камеры нет: чёрный кадр + UI поверх (путь `Content::Black` при установленном источнике UI).
        let recording = pipelines.record_black_frame(target, None)?;
        assert_eq!(recording.screen, ScreenContent::Black { compose_failure: None });
        let failures = pipelines.record_ui_overlay(
            target,
            &mut UiOverlayFrame {
                state: &mut state,
                present_time: MonotonicNanos::from_nanos(1_000),
            },
            &recording.screen,
            SubmissionSerial(1),
        );
        assert_eq!(failures, 0, "метки времени записаны");
        Ok(())
    });
    assert_eq!(calls.load(Ordering::SeqCst), 1, "источник вызван ровно раз");
    let center = pixel(&image, 50, 100);
    assert!(
        center[0] > 200 && center[1] > 200 && center[2] > 200,
        "круг UI в центре: {center:?}"
    );
    assert_eq!(pixel(&image, 5, 5), BLACK, "остальное — чёрное");
    assert_eq!(state.snapshot().frames_drawn, 1);
    context.wait_idle().expect("простой GPU");
    assert_eq!(context.validation_report().errors, 0);
}

#[test]
fn a_failing_ui_source_leaves_the_viewfinder_on_screen_and_is_counted() {
    let Some(context) = context_or_skip() else { return };
    let mut stand = Stand::new(&context);
    let texture = quadrant_texture(&context, 8, 8);
    let (mut state, calls) = install_disc(true);
    stand.pipelines.prepare_ui_overlay().expect("рендер UI");
    let image = stand.frame(|pipelines, target| {
        let destination = pipelines.compose_view(target, texture.sampled_view(), rect(0.0, 0.0, 1.0, 0.5))?;
        let failures = pipelines.record_ui_overlay(
            target,
            &mut UiOverlayFrame {
                state: &mut state,
                present_time: MonotonicNanos::from_nanos(1_000),
            },
            &ScreenContent::Viewfinder { destination },
            SubmissionSerial(1),
        );
        assert_eq!(failures, 0);
        Ok(())
    });
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let snapshot = state.snapshot();
    assert_eq!(
        (snapshot.source_failures, snapshot.frames_drawn),
        (1, 0),
        "{snapshot:?}"
    );
    assert_color(&image, 20, 20, [255, 0, 0]);
    assert_color(&image, 80, 90, [255, 255, 255]);
    assert_eq!(pixel(&image, 50, 150), BLACK);
}

#[test]
fn without_an_installed_source_the_black_frame_path_records_no_ui() {
    let Some(context) = context_or_skip() else { return };
    let mut stand = Stand::new(&context);
    let mut state = UiOverlayState::new(8);
    let image = stand.frame(|pipelines, target| {
        let recording = pipelines.record_black_frame(target, None)?;
        let failures = pipelines.record_ui_overlay(
            target,
            &mut UiOverlayFrame {
                state: &mut state,
                present_time: MonotonicNanos::from_nanos(1_000),
            },
            &recording.screen,
            SubmissionSerial(1),
        );
        assert_eq!(failures, 0);
        Ok(())
    });
    assert_eq!(pixel(&image, 50, 100), BLACK, "UI не включён: чёрный кадр как прежде");
    assert_eq!(state.snapshot().frames_drawn, 0);
}
