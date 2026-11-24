//! GPU-тесты на цели, которую можно ПРОЧИТАТЬ: основание пропуска тиков без изменений (решение 214, сессия 16a).
//! Пропуск законен, только если повторная запись того же кадра даёт те же пиксели. Здесь это проверяется на пикселях:
//! * кадр с равным списком UI поверх той же картинки побитно совпадает с прошлым (стекло и фигуры, два списка,
//!   построенных заново — как источник строит их на каждом тике);
//! * контроль чувствительности: другой список, другой свет и другой фон дают другие пиксели, то есть тест
//!   различает «то же» и «не то же».

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

use std::sync::{Arc, Mutex, PoisonError};

use ash::vk;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::sampled_texture::{SampledTexture, TextureDesc, TextureFormat};
use shell_contract::clock::MonotonicNanos;
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{DrawItem, DrawList, GlassGroupItem, GlassMember, PressState, ShapeItem};
use ui_draw::frame_source::{BackdropContinuity, UiFrameError, UiFrameRequest, UiFrameSource};
use ui_draw::geometry::PxSize;
use ui_draw::glass::GlassStyle;
use ui_draw::light::LightDirection;
use ui_draw::shape::ShapeGeometry;
use ui_draw::transform::Affine2;

use super::super::screen_tests::{Stand, context_or_skip, rect};
use super::super::{CameraPipelines, FrameTarget, ScreenContent};
use crate::config::ScopesConfig;
use crate::ui_overlay::{UiOverlayCommand, UiOverlayOutcome};
use crate::vulkan_backend::camera::SubmissionSerial;
use crate::vulkan_backend::camera::ui_state::{UiOverlayFrame, UiOverlayState};

/// Источник UI, отдающий каждый раз ЗАНОВО построенный список по параметрам теста (как реальный источник на тике).
struct RebuildingSource {
    light_degrees: Arc<Mutex<f32>>,
}

impl UiFrameSource for RebuildingSource {
    fn name(&self) -> &'static str {
        "rebuilding-unchanged-test"
    }

    fn produce_frame(&mut self, request: &UiFrameRequest) -> Result<DrawList, UiFrameError> {
        let light = *self.light_degrees.lock().unwrap_or_else(PoisonError::into_inner);
        let mut list = DrawList::new(LightDirection::from_angle_degrees(light));
        let capsule = PxSize::new(request.logical_size.width * 0.6, request.logical_size.height * 0.15);
        list.items.push(DrawItem::Glass(GlassGroupItem {
            params: GlassStyle::default().resolve(capsule, request.px_per_pt),
            members: vec![GlassMember {
                geometry: ShapeGeometry::Capsule { size: capsule },
                transform: Affine2::translate(request.logical_size.width * 0.5, request.logical_size.height * 0.4),
                press: PressState::RELEASED,
            }],
            merge_radius_px: 0.0,
            effect: 1.0,
            opacity: Opacity::OPAQUE,
        }));
        list.items.push(DrawItem::Shape(ShapeItem {
            geometry: ShapeGeometry::Circle { diameter: 30.0 },
            transform: Affine2::translate(request.logical_size.width * 0.5, request.logical_size.height * 0.75),
            fill: DisplayColor::WHITE,
            stroke: None,
            shadow: None,
            opacity: Opacity::OPAQUE,
        }));
        Ok(list)
    }
}

/// Текстура-фон видоискателя с градиентом (стеклу есть что преломлять и размывать). `blue` — вторая «картинка камеры».
fn backdrop_texture(context: &GpuContext, blue: bool) -> SampledTexture {
    let texels: Vec<u8> = (0..8 * 8)
        .flat_map(|index| {
            let column = (index % 8) as u8;
            if blue {
                [0u8, column * 20, 255, 255]
            } else {
                [column * 30, 80, 40, 255]
            }
        })
        .collect();
    SampledTexture::upload(
        context,
        &TextureDesc {
            extent: vk::Extent2D { width: 8, height: 8 },
            format: TextureFormat::Rgba8Unorm,
            label: "фон",
        },
        &texels,
    )
    .expect("текстура загружена")
}

/// Один кадр: компоновка текстуры на всю цель, затем UI источника состояния `state` (подготовка и запись по шагам,
/// как в сессии показа).
fn frame(stand: &mut Stand, texture: &SampledTexture, state: &mut UiOverlayState) -> gpu_vulkan::offscreen::Rgba8Image {
    stand.frame(|pipelines: &mut CameraPipelines, target: &FrameTarget| {
        let destination = pipelines.compose_view(target, texture.sampled_view(), rect(0.0, 0.0, 1.0, 1.0))?;
        let mut ui = UiOverlayFrame {
            state,
            present_time: MonotonicNanos::from_nanos(1_000),
        };
        let tick = pipelines.prepare_ui_tick(target, &mut ui, BackdropContinuity::Live);
        let outcome = pipelines.record_prepared_ui(
            target,
            &mut ui,
            &tick,
            &ScreenContent::Viewfinder { destination },
            SubmissionSerial(1),
        );
        assert!(outcome.drawn, "UI записан целиком");
        Ok(())
    })
}

fn state_with_light(light_degrees: f32) -> (UiOverlayState, Arc<Mutex<f32>>) {
    let light = Arc::new(Mutex::new(light_degrees));
    let mut state = UiOverlayState::new(8);
    let outcome = state.apply(UiOverlayCommand::Install(Box::new(RebuildingSource {
        light_degrees: Arc::clone(&light),
    })));
    assert_eq!(outcome, UiOverlayOutcome::Applied);
    (state, light)
}

#[test]
fn a_frame_with_an_equal_rebuilt_ui_over_the_same_picture_is_bit_identical_to_the_previous_one() {
    let Some(context) = context_or_skip() else { return };
    let mut stand = Stand::with_scopes(&context, &ScopesConfig::default());
    let texture = backdrop_texture(&context, false);
    stand.pipelines.prepare_ui_overlay().expect("рендер UI");
    let (mut state, _light) = state_with_light(45.0);

    // Три кадра подряд: первый (после него эталон) и два повтора. Источник строит список заново на каждом кадре.
    let first = frame(&mut stand, &texture, &mut state);
    let second = frame(&mut stand, &texture, &mut state);
    let third = frame(&mut stand, &texture, &mut state);
    assert_eq!(
        second, first,
        "повтор не меняет ни одного пикселя: пропуск тика законен"
    );
    assert_eq!(third, first);
    context.wait_idle().expect("простой GPU");
    assert_eq!(context.validation_report().errors, 0);
}

#[test]
fn a_changed_list_light_or_backdrop_changes_the_pixels_so_the_comparison_above_is_not_vacuous() {
    let Some(context) = context_or_skip() else { return };
    let mut stand = Stand::with_scopes(&context, &ScopesConfig::default());
    let texture = backdrop_texture(&context, false);
    let other_picture = backdrop_texture(&context, true);
    stand.pipelines.prepare_ui_overlay().expect("рендер UI");
    let (mut state, light) = state_with_light(45.0);

    let baseline = frame(&mut stand, &texture, &mut state);
    *light.lock().unwrap_or_else(PoisonError::into_inner) = 135.0;
    let other_light = frame(&mut stand, &texture, &mut state);
    assert_ne!(other_light, baseline, "свет от наклона меняет блик стекла");
    *light.lock().unwrap_or_else(PoisonError::into_inner) = 45.0;
    let other_backdrop = frame(&mut stand, &other_picture, &mut state);
    assert_ne!(
        other_backdrop, baseline,
        "новая картинка камеры под стеклом меняет кадр"
    );
    let back = frame(&mut stand, &texture, &mut state);
    assert_eq!(
        back, baseline,
        "вернули те же входы — вернулись те же пиксели (нет скрытого накопления)"
    );
}
