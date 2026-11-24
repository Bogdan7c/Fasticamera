//! GPU-тесты подавленного рисования UI (замер A/B, 16a) на offscreen-цели, которую можно ПРОЧИТАТЬ: кадр с подавленным
//! UI побитно равен кадру «только компоновщик», а источник при этом получил свой кадр. Контрольный прогон без подавления
//! доказывает, что тест различает «UI нарисован» и «UI не нарисован».

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ash::vk;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::sampled_texture::{SampledTexture, TextureDesc, TextureFormat};
use shell_contract::clock::MonotonicNanos;
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{DrawItem, DrawList, ShapeItem};
use ui_draw::frame_source::{BackdropContinuity, UiFrameError, UiFrameRequest, UiFrameSource};
use ui_draw::light::LightDirection;
use ui_draw::shape::ShapeGeometry;
use ui_draw::transform::Affine2;

use super::super::screen_tests::{Stand, context_or_skip, pixel, rect};
use super::super::{CameraPipelines, FrameTarget, ScreenContent};
use crate::config::ScopesConfig;
use crate::ui_overlay::{UiOverlayCommand, UiOverlayOutcome};
use crate::vulkan_backend::camera::SubmissionSerial;
use crate::vulkan_backend::camera::ui_state::{UiOverlayFrame, UiOverlayState};

/// Источник UI: большой белый круг в центре цели; считает вызовы, `failing` — отвечает отказом.
struct DiscSource {
    calls: Arc<AtomicU64>,
    failing: bool,
}

impl UiFrameSource for DiscSource {
    fn name(&self) -> &'static str {
        "disc-suppression-test"
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

/// Состояние UI с установленным источником-кругом и счётчик его вызовов.
fn state_with_disc(failing: bool) -> (UiOverlayState, Arc<AtomicU64>) {
    let calls = Arc::new(AtomicU64::new(0));
    let mut state = UiOverlayState::new(8);
    let outcome = state.apply(UiOverlayCommand::Install(Box::new(DiscSource {
        calls: Arc::clone(&calls),
        failing,
    })));
    assert_eq!(outcome, UiOverlayOutcome::Applied);
    (state, calls)
}

/// Синяя текстура 8×8: фон видоискателя для компоновщика (белый круг UI на ней виден).
fn blue_texture(context: &GpuContext) -> SampledTexture {
    let texels: Vec<u8> = (0..8 * 8).flat_map(|_| [0u8, 0, 255, 255]).collect();
    SampledTexture::upload(
        context,
        &TextureDesc {
            extent: vk::Extent2D { width: 8, height: 8 },
            format: TextureFormat::Rgba8Unorm,
            label: "синяя",
        },
        &texels,
    )
    .expect("текстура загружена")
}

/// Что делает UI на кадре: записывается, подавлен или кадра UI нет вовсе (эталон «только компоновщик»).
enum UiOnFrame<'a> {
    Recorded(&'a mut UiOverlayState),
    Suppressed(&'a mut UiOverlayState),
    Nothing,
}

/// Кадр: компоновщик кладёт синюю текстуру на всю цель, затем UI по режиму `ui_on_frame`.
fn compose_frame(
    stand: &mut Stand,
    texture: &SampledTexture,
    ui_on_frame: UiOnFrame<'_>,
) -> gpu_vulkan::offscreen::Rgba8Image {
    stand.frame(|pipelines, target| {
        let destination = pipelines.compose_view(target, texture.sampled_view(), rect(0.0, 0.0, 1.0, 1.0))?;
        match ui_on_frame {
            UiOnFrame::Recorded(state) => {
                let failures = record_ui(pipelines, target, state, destination);
                assert_eq!(failures, 0, "метки времени записаны");
            }
            UiOnFrame::Suppressed(state) => pipelines.skip_ui_overlay_drawing(
                target,
                &mut UiOverlayFrame {
                    state,
                    present_time: MonotonicNanos::from_nanos(1_000),
                },
                BackdropContinuity::Live,
            ),
            UiOnFrame::Nothing => {}
        }
        Ok(())
    })
}

fn record_ui(
    pipelines: &mut CameraPipelines,
    target: &FrameTarget,
    state: &mut UiOverlayState,
    destination: gpu_vulkan::compositor::LogicalRect,
) -> u64 {
    pipelines.record_ui_overlay(
        target,
        &mut UiOverlayFrame {
            state,
            present_time: MonotonicNanos::from_nanos(1_000),
        },
        &ScreenContent::Viewfinder { destination },
        SubmissionSerial(1),
        BackdropContinuity::Live,
    )
}

#[test]
fn a_suppressed_ui_frame_equals_the_compositor_only_frame_while_the_source_still_gets_its_frame() {
    let Some(context) = context_or_skip() else { return };
    let mut stand = Stand::with_scopes(&context, &ScopesConfig::default());
    let texture = blue_texture(&context);
    stand.pipelines.prepare_ui_overlay().expect("рендер UI");
    let (mut state, calls) = state_with_disc(false);

    let compositor_only = compose_frame(&mut stand, &texture, UiOnFrame::Nothing);
    let suppressed = compose_frame(&mut stand, &texture, UiOnFrame::Suppressed(&mut state));
    assert_eq!(
        suppressed, compositor_only,
        "подавленный UI не оставил на кадре ни пикселя"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1, "источник получил свой кадр");
    let after_suppressed = state.snapshot();
    assert_eq!(
        (
            after_suppressed.frames_suppressed,
            after_suppressed.frames_drawn,
            after_suppressed.frames_failed
        ),
        (1, 0, 0),
        "{after_suppressed:?}"
    );
    assert_eq!(
        pixel(&suppressed, 50, 100),
        [0, 0, 255, 255],
        "в центре только синий компоновщик"
    );

    // Контроль: тот же источник без подавления рисует круг — тест различает «нарисован» и «не нарисован».
    let drawn = compose_frame(&mut stand, &texture, UiOnFrame::Recorded(&mut state));
    assert_ne!(drawn, compositor_only);
    let center = pixel(&drawn, 50, 100);
    assert!(center[0] > 200 && center[1] > 200, "круг UI в центре: {center:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let after_drawn = state.snapshot();
    assert_eq!((after_drawn.frames_suppressed, after_drawn.frames_drawn), (1, 1));

    // Подавление снова: UI опять исчезает с кадра.
    let suppressed_again = compose_frame(&mut stand, &texture, UiOnFrame::Suppressed(&mut state));
    assert_eq!(suppressed_again, compositor_only);
    assert_eq!(state.snapshot().frames_suppressed, 2);
    context.wait_idle().expect("простой GPU");
    assert_eq!(context.validation_report().errors, 0);
}

#[test]
fn suppressed_frame_does_not_create_the_ui_renderer() {
    let Some(context) = context_or_skip() else { return };
    let mut stand = Stand::with_scopes(&context, &ScopesConfig::default());
    let texture = blue_texture(&context);
    let (mut state, calls) = state_with_disc(false);
    // Рендер UI не подготовлен: подавленный кадр не должен его создавать (это стоило бы кадру конвейеров).
    let before = context.live_objects();
    compose_frame(&mut stand, &texture, UiOnFrame::Suppressed(&mut state));
    compose_frame(&mut stand, &texture, UiOnFrame::Suppressed(&mut state));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(state.snapshot().frames_suppressed, 2);
    let after = context.live_objects();
    assert_eq!(
        after.differences_from(&before),
        Vec::new(),
        "подавленные кадры не создали объектов GPU"
    );
}

#[test]
fn suppressed_frame_counts_nothing_without_a_source_and_reports_source_failure_honestly() {
    let Some(context) = context_or_skip() else { return };
    let mut stand = Stand::with_scopes(&context, &ScopesConfig::default());
    let texture = blue_texture(&context);

    // Источника нет: ни вызовов, ни счётчиков.
    let mut empty = UiOverlayState::new(8);
    compose_frame(&mut stand, &texture, UiOnFrame::Suppressed(&mut empty));
    assert_eq!(empty.snapshot().frames_suppressed, 0);

    // Источник отказал: отказ посчитан как отказ источника, а не как подавленный кадр.
    let (mut failing, calls) = state_with_disc(true);
    compose_frame(&mut stand, &texture, UiOnFrame::Suppressed(&mut failing));
    assert_eq!(calls.load(Ordering::SeqCst), 1, "источник вызван и под подавлением");
    let snapshot = failing.snapshot();
    assert_eq!(
        (snapshot.source_failures, snapshot.frames_suppressed),
        (1, 0),
        "{snapshot:?}"
    );
}
