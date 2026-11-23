//! Общие помощники GPU-тестов рендера UI: контекст, стенд «цель + рендер», построители списка рисования
//! и CPU-зеркало (см. `mirror`).

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "общий модуль подключается разными тестовыми файлами и использует не всё; в тестах падение — способ сообщить о провале"
)]

pub mod image;
pub mod mirror;
pub mod pictures;

use ash::vk;
use gpu_vulkan::color_target::{
    ColorTarget, ColorTargetDesc, ColorTargetFormat, ColorTargetPreviousUse, ContentOnPrepare,
};
use gpu_vulkan::context::{GpuContext, GpuContextConfig};
use gpu_vulkan::device::DeviceProfile;
use gpu_vulkan::kit_test_support::{read_color_target_rgba, run_one_shot_blocking};
use gpu_vulkan::raster::{AttachmentLoad, RasterPass};
use gpu_vulkan::test_pattern::SurfaceTransform;
use gpu_vulkan::validation::ValidationMode;
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{DrawItem, DrawList, GlassGroupItem, GlassMember, PressState, ShapeItem};
use ui_draw::geometry::{PxPoint, PxSize};
use ui_draw::glass::GlassStyle;
use ui_draw::light::LightDirection;
use ui_draw::shape::ShapeGeometry;
use ui_draw::transform::Affine2;
use ui_render_vulkan::{
    BackdropInput, LiveImages, UiFrame, UiFrameReport, UiRenderError, UiRenderer, UiRendererConfig,
};

const SKIP_ENV: &str = "FASTICAMERA_SKIP_GPU_TESTS";

/// Контекст `Headless` с полной validation или `None` при осознанном пропуске (как у gpu-vulkan).
pub fn gpu_context_or_skip() -> Option<GpuContext> {
    let config = GpuContextConfig::new(DeviceProfile::Headless, ValidationMode::FULL);
    match GpuContext::new(&config) {
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

/// GPU простаивает и validation не сообщила ни одной ошибки.
pub fn assert_no_validation_errors(context: &GpuContext) {
    context.wait_idle().expect("wait_idle в конце теста");
    let report = context.validation_report();
    assert_eq!(report.errors, 0, "validation нашла ошибки: {report:?}");
}

/// Цвет очистки цели по умолчанию (чтобы отличать «не нарисовано» от чёрного).
pub const CLEAR: [f32; 4] = [0.1, 0.1, 0.15, 1.0];

/// Стенд: цель нужного формата/размера + рендер. Каждый `render` — блокирующая отправка и readback.
pub struct Rig {
    pub context: GpuContext,
    pub renderer: UiRenderer,
    pub target: ColorTarget,
    pub logical: PxSize,
    pub transform: SurfaceTransform,
    pub format: ColorTargetFormat,
    next_slot: usize,
}

impl Rig {
    pub fn new(
        context: &GpuContext,
        logical: (u32, u32),
        transform: SurfaceTransform,
        format: ColorTargetFormat,
    ) -> Self {
        Self::with_config(context, logical, transform, format, |_| {})
    }

    pub fn with_config(
        context: &GpuContext,
        logical: (u32, u32),
        transform: SurfaceTransform,
        format: ColorTargetFormat,
        tweak: impl FnOnce(&mut UiRendererConfig),
    ) -> Self {
        let extent = if transform.swaps_extent() {
            vk::Extent2D {
                width: logical.1,
                height: logical.0,
            }
        } else {
            vk::Extent2D {
                width: logical.0,
                height: logical.1,
            }
        };
        let target = ColorTarget::new(
            context,
            &ColorTargetDesc {
                extent,
                format,
                label: "ui-test-target",
            },
        )
        .unwrap();
        let mut config = UiRendererConfig::new(format.vk_format(), 2);
        tweak(&mut config);
        let renderer = UiRenderer::new(context, &config).unwrap();
        Self {
            context: context.clone(),
            renderer,
            target,
            logical: PxSize::new(logical.0 as f32, logical.1 as f32),
            transform,
            format,
            next_slot: 0,
        }
    }

    pub fn frame<'a>(&self, draw_list: &'a DrawList, backdrop: BackdropInput<'a>) -> UiFrame<'a> {
        self.frame_with_live(draw_list, backdrop, LiveImages::none())
    }

    /// Кадр с таблицей живых картинок (приборы цвета 14h).
    pub fn frame_with_live<'a>(
        &self,
        draw_list: &'a DrawList,
        backdrop: BackdropInput<'a>,
        live_images: LiveImages<'a>,
    ) -> UiFrame<'a> {
        UiFrame {
            draw_list,
            logical_size: self.logical,
            surface_transform: self.transform,
            backdrop,
            live_images,
        }
    }

    /// Рисует кадр поверх залитой `clear` цели и читает её. Ошибка рендера возвращается вместе с пикселями цели.
    pub fn render_with_clear(
        &mut self,
        draw_list: &DrawList,
        backdrop: BackdropInput<'_>,
        clear: [f32; 4],
    ) -> (Result<UiFrameReport, UiRenderError>, Vec<[f32; 4]>) {
        self.render_with_live_images(draw_list, backdrop, clear, LiveImages::none())
    }

    /// То же с таблицей живых картинок кадра.
    pub fn render_with_live_images(
        &mut self,
        draw_list: &DrawList,
        backdrop: BackdropInput<'_>,
        clear: [f32; 4],
        live_images: LiveImages<'_>,
    ) -> (Result<UiFrameReport, UiRenderError>, Vec<[f32; 4]>) {
        let slot = self.next_slot;
        self.next_slot = (self.next_slot + 1) % self.renderer.config().frame_slots;
        let frame = self.frame_with_live(draw_list, backdrop, live_images);
        let mut outcome = None;
        let renderer = &mut self.renderer;
        let target = &self.target;
        let context = &self.context;
        run_one_shot_blocking(context, |command_buffer| {
            target.record_prepare_for_render(
                command_buffer,
                ColorTargetPreviousUse::NotUsedYet,
                ContentOnPrepare::Discard,
            )?;
            RasterPass::begin(
                context,
                command_buffer,
                target.attachment(),
                AttachmentLoad::Clear(clear),
            )?
            .end();
            outcome = Some(renderer.record_frame(command_buffer, slot, target.attachment(), &frame));
            Ok(())
        })
        .unwrap();
        self.renderer.release_slot_after_fence(slot).unwrap();
        let pixels = read_color_target_rgba(&self.target, ColorTargetPreviousUse::Rendered).unwrap();
        (outcome.unwrap(), pixels)
    }

    /// Успешный кадр: отчёт и пиксели.
    pub fn render(&mut self, draw_list: &DrawList, backdrop: BackdropInput<'_>) -> (UiFrameReport, Vec<[f32; 4]>) {
        let (result, pixels) = self.render_with_clear(draw_list, backdrop, CLEAR);
        (result.unwrap(), pixels)
    }

    /// Меняет логический размер экрана: новая цель, тот же рендер (проверка пересоздания пирамиды).
    pub fn resize(&mut self, logical: (u32, u32)) {
        let extent = if self.transform.swaps_extent() {
            vk::Extent2D {
                width: logical.1,
                height: logical.0,
            }
        } else {
            vk::Extent2D {
                width: logical.0,
                height: logical.1,
            }
        };
        self.target = ColorTarget::new(
            &self.context,
            &ColorTargetDesc {
                extent,
                format: self.format,
                label: "ui-test-target",
            },
        )
        .unwrap();
        self.logical = PxSize::new(logical.0 as f32, logical.1 as f32);
    }

    /// Как в боевом кадре: композитор кладёт кадр видоискателя в цель, затем поверх рисуется UI с тем же фоном.
    pub fn render_over_composited_backdrop(
        &mut self,
        draw_list: &DrawList,
        source: &gpu_vulkan::backdrop::BackdropSource,
    ) -> Vec<[f32; 4]> {
        let slot = self.next_slot;
        self.next_slot = (self.next_slot + 1) % self.renderer.config().frame_slots;
        let logical_extent = vk::Extent2D {
            width: self.logical.width as u32,
            height: self.logical.height as u32,
        };
        let placement = full_screen_placement(self.logical.width, self.logical.height);
        let frame = self.frame(draw_list, BackdropInput::Live { source, placement });
        let mut compositor =
            gpu_vulkan::compositor::ImageCompositor::new(&self.context, self.format.vk_format(), 2).unwrap();
        let mut outcome = None;
        let renderer = &mut self.renderer;
        let target = &self.target;
        let context = &self.context;
        let transform = self.transform;
        run_one_shot_blocking(context, |command_buffer| {
            target.record_prepare_for_render(
                command_buffer,
                ColorTargetPreviousUse::NotUsedYet,
                ContentOnPrepare::Discard,
            )?;
            compositor.record(
                command_buffer,
                gpu_vulkan::test_pattern::PatternTarget {
                    view: target.sampled_view().view,
                    extent: target.extent(),
                },
                &gpu_vulkan::compositor::CompositeRequest {
                    source: source.view(),
                    transform,
                    destination: gpu_vulkan::compositor::LogicalRect::covering(logical_extent),
                    background: gpu_vulkan::compositor::BackgroundColor::BLACK,
                    frame_slot: slot,
                },
            )?;
            outcome = Some(renderer.record_frame(command_buffer, slot, target.attachment(), &frame));
            Ok(())
        })
        .unwrap();
        outcome.unwrap().unwrap();
        compositor.release_slot_after_fence(slot).unwrap();
        self.renderer.release_slot_after_fence(slot).unwrap();
        read_color_target_rgba(&self.target, ColorTargetPreviousUse::Rendered).unwrap()
    }

    pub fn width(&self) -> u32 {
        self.target.extent().width
    }
}

/// Пиксель `(x, y)` построчного readback.
pub fn pixel(pixels: &[[f32; 4]], width: u32, x: u32, y: u32) -> [f32; 4] {
    pixels[(y * width + x) as usize]
}

/// Центр пикселя в экранных px.
pub fn center_of(x: u32, y: u32) -> PxPoint {
    PxPoint::new(x as f32 + 0.5, y as f32 + 0.5)
}

pub fn empty_list() -> DrawList {
    DrawList::new(LightDirection::from_angle_degrees(45.0))
}

pub fn shape_item(geometry: ShapeGeometry, transform: Affine2, fill: DisplayColor) -> ShapeItem {
    ShapeItem {
        geometry,
        transform,
        fill,
        stroke: None,
        shadow: None,
        opacity: Opacity::OPAQUE,
    }
}

pub fn shape(item: ShapeItem) -> DrawItem {
    DrawItem::Shape(item)
}

/// Группа стекла из форм `(геометрия, центр на экране)` со стилем по умолчанию, пересчитанным в px (3 px/pt).
pub fn glass_group(style: &GlassStyle, members: &[(ShapeGeometry, PxPoint)], merge_radius_px: f32) -> GlassGroupItem {
    let size = members[0].0.extent();
    GlassGroupItem {
        params: style.resolve(size, 3.0),
        members: members
            .iter()
            .map(|(geometry, center)| GlassMember {
                geometry: *geometry,
                transform: Affine2::translate(center.x, center.y),
                press: PressState::RELEASED,
            })
            .collect(),
        merge_radius_px,
        effect: 1.0,
        opacity: Opacity::OPAQUE,
    }
}

/// Стиль «чистая линза»: размытие, тон, блик, затемнение, насыщенность, тёмный край, тень, свечение нулевые;
/// остаются только преломление и дисперсия (для проверки координат выборки).
pub fn lens_only_style() -> GlassStyle {
    let default = GlassStyle::default();
    GlassStyle {
        blur_radius_pt: 0.0,
        saturation: 1.0,
        tint_level: 0.0,
        dimming: ui_draw::glass::DimmingStyle {
            max_alpha: 0.0,
            ..default.dimming
        },
        specular: ui_draw::glass::SpecularStyle {
            alpha: 0.0,
            ..default.specular
        },
        dark_edge: ui_draw::glass::DarkEdgeStyle {
            strength: 0.0,
            ..default.dark_edge
        },
        shadow: ui_draw::glass::ShadowStyle {
            alpha: 0.0,
            ..default.shadow
        },
        press: ui_draw::glass::PressStyle {
            glow_alpha: 0.0,
            ..default.press
        },
        ..default
    }
}

/// Пиксель линейной «сетки координат» (как у `BackdropPattern::CoordinateGrid`) в экранной точке.
pub fn grid_color(point: PxPoint, width: f32, height: f32) -> DisplayColor {
    let x = (point.x / width).clamp(0.0, 1.0);
    DisplayColor::rgb(x, (point.y / height).clamp(0.0, 1.0), x)
}

/// Живой фон на весь экран из синтетической подложки.
pub fn full_screen_placement(width: f32, height: f32) -> ui_render_vulkan::BackdropPlacement {
    ui_render_vulkan::BackdropPlacement {
        destination: ui_draw::geometry::PxRect::new(PxPoint::new(0.0, 0.0), PxSize::new(width, height)),
    }
}

/// Средняя яркость под группой по правилу шейдера: 5 выборок (центр и четверти границ) от яркости фона.
pub fn mean_luma_under(group: &GlassGroupItem, backdrop: impl Fn(PxPoint) -> DisplayColor) -> f32 {
    let bounds = group.bounds_px().unwrap();
    let center = bounds.center();
    let (qx, qy) = (bounds.size.width * 0.25, bounds.size.height * 0.25);
    let taps = [(0.0, 0.0), (qx, qy), (-qx, qy), (qx, -qy), (-qx, -qy)];
    taps.iter()
        .map(|(dx, dy)| ui_draw::color::luma_rec709(backdrop(PxPoint::new(center.x + dx, center.y + dy))))
        .sum::<f32>()
        / 5.0
}
