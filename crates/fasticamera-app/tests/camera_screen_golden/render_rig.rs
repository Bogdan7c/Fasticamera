//! Настоящий рендер списка рисования экрана на GPU ПК: `UiRenderer` поверх фиксированного фона.
//!
//! Порядок как в бою: композитор кладёт «кадр видоискателя» (синтетическое «фото») в цель, затем поверх рисуется UI,
//! и стекло берёт фон из того же источника. Цель — `A2B10G10R10` (формат экрана приложения), снимок читается обратно.

use ash::vk;
use gpu_vulkan::backdrop::BackdropSource;
use gpu_vulkan::color_target::{
    ColorTarget, ColorTargetDesc, ColorTargetFormat, ColorTargetPreviousUse, ContentOnPrepare,
};
use gpu_vulkan::compositor::{BackgroundColor, CompositeRequest, ImageCompositor, LogicalRect};
use gpu_vulkan::context::{GpuContext, GpuContextConfig};
use gpu_vulkan::device::DeviceProfile;
use gpu_vulkan::kit_test_support::{read_color_target_rgba, run_one_shot_blocking};
use gpu_vulkan::test_pattern::{PatternTarget, SurfaceTransform};
use gpu_vulkan::validation::ValidationMode;
use ui_draw::draw_list::DrawList;
use ui_draw::geometry::{PxPoint, PxRect, PxSize};
use ui_render_vulkan::testing::{BackdropPattern, SyntheticBackdrop};
use ui_render_vulkan::{BackdropInput, BackdropPlacement, UiFrame, UiRenderer, UiRendererConfig};

use crate::driver::{HEIGHT_PX, WIDTH_PX};

/// Явный пропуск GPU-тестов (как у `gpu-vulkan` и `ui-render-vulkan`).
const SKIP_ENV: &str = "FASTICAMERA_SKIP_GPU_TESTS";
/// Слотов кадров рендера и композитора: один блокирующий кадр за раз, двух достаточно.
const FRAME_SLOTS: usize = 2;

/// Где на экране лежит «кадр видоискателя» (целые px): за его границей фон чёрный, как в бою.
#[derive(Debug, Clone, Copy)]
pub struct ViewfinderRectPx {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Размер синтетического «фото»: 4:3 портрет на всю ширину экрана. В прямоугольник другого соотношения оно
/// растягивается — для golden важна фиксированность картинки, а не её пропорции.
const BACKDROP_HEIGHT_PX: u32 = 1440;

/// Снимок кадра: пиксели RGBA float 0..1, построчно сверху вниз, `WIDTH_PX × HEIGHT_PX`.
pub struct Snapshot {
    pub pixels: Vec<[f32; 4]>,
}

impl Snapshot {
    /// Пиксель `(x, y)` в px экрана.
    pub fn at(&self, x: u32, y: u32) -> [f32; 3] {
        let pixel = self.pixels[(y * WIDTH_PX + x) as usize];
        [pixel[0], pixel[1], pixel[2]]
    }

    /// Яркость (Rec.709) пикселя `(x, y)`.
    pub fn luma(&self, x: u32, y: u32) -> f32 {
        let [r, g, b] = self.at(x, y);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    /// Сколько пикселей окна `[x0, x1) × [y0, y1)` удовлетворяют условию.
    pub fn count_in(&self, window: (u32, u32, u32, u32), predicate: impl Fn([f32; 3]) -> bool) -> usize {
        let (x0, y0, x1, y1) = window;
        let mut count = 0;
        for y in y0..y1.min(HEIGHT_PX) {
            for x in x0..x1.min(WIDTH_PX) {
                if predicate(self.at(x, y)) {
                    count += 1;
                }
            }
        }
        count
    }

    /// Средняя абсолютная разница по каналам в окне между двумя снимками.
    pub fn mean_difference_in(&self, other: &Snapshot, window: (u32, u32, u32, u32)) -> f32 {
        let (x0, y0, x1, y1) = window;
        let (mut sum, mut count) = (0.0_f32, 0_u32);
        for y in y0..y1.min(HEIGHT_PX) {
            for x in x0..x1.min(WIDTH_PX) {
                let (a, b) = (self.at(x, y), other.at(x, y));
                sum += (0..3).map(|c| (a[c] - b[c]).abs()).sum::<f32>() / 3.0;
                count += 1;
            }
        }
        sum / count.max(1) as f32
    }
}

/// Контекст GPU или `None` при осознанном пропуске (`FASTICAMERA_SKIP_GPU_TESTS=1`); иначе недоступный GPU — паника.
pub fn gpu_context_or_skip() -> Option<GpuContext> {
    let config = GpuContextConfig::new(DeviceProfile::Headless, ValidationMode::FULL);
    match GpuContext::new(&config) {
        Ok(context) => Some(context),
        Err(error) if std::env::var(SKIP_ENV).as_deref() == Ok("1") => {
            eprintln!("ПРОПУСК GPU-теста: {error} ({SKIP_ENV}=1)");
            None
        }
        Err(error) => {
            panic!("GPU недоступен: {error}. Установите {SKIP_ENV}=1, чтобы пропустить GPU-тесты осознанно")
        }
    }
}

/// GPU простаивает и validation не сообщила ни одной ошибки.
pub fn assert_no_validation_errors(context: &GpuContext) {
    context.wait_idle().expect("wait_idle в конце теста");
    let report = context.validation_report();
    assert_eq!(report.errors, 0, "validation нашла ошибки: {report:?}");
}

/// Стенд: цель экрана, рендер UI и фиксированное синтетическое «фото» под ним.
pub struct RenderRig {
    context: GpuContext,
    renderer: UiRenderer,
    target: ColorTarget,
    backdrop: SyntheticBackdrop,
    slot: usize,
}

impl RenderRig {
    pub fn new(context: &GpuContext) -> Self {
        let extent = vk::Extent2D {
            width: WIDTH_PX,
            height: HEIGHT_PX,
        };
        let format = ColorTargetFormat::A2B10G10R10Unorm;
        let target = ColorTarget::new(
            context,
            &ColorTargetDesc {
                extent,
                format,
                label: "camera-screen-golden-target",
            },
        )
        .expect("цель экрана");
        // Основные цвета — как у экрана приложения (Display P3): картинки UI (миниатюра, 14g) переводятся в них.
        let mut renderer_config = UiRendererConfig::new(format.vk_format(), FRAME_SLOTS);
        renderer_config.display_primaries = ui_draw::color::DisplayPrimaries::DisplayP3;
        let renderer = UiRenderer::new(context, &renderer_config).expect("рендер UI");
        let backdrop_extent = vk::Extent2D {
            width: WIDTH_PX,
            height: BACKDROP_HEIGHT_PX,
        };
        let backdrop =
            SyntheticBackdrop::new(context, backdrop_extent, BackdropPattern::Photo).expect("синтетический фон");
        Self {
            context: context.clone(),
            renderer,
            target,
            backdrop,
            slot: 0,
        }
    }

    /// Рисует список поверх «фото» в прямоугольнике `viewfinder` и читает цель обратно. Любая ошибка рендера — паника теста.
    pub fn render(&mut self, list: &DrawList, viewfinder: ViewfinderRectPx) -> Snapshot {
        let slot = self.slot;
        self.slot = (self.slot + 1) % FRAME_SLOTS;
        let source: BackdropSource = self.backdrop.source(1);
        let logical = PxSize::new(WIDTH_PX as f32, HEIGHT_PX as f32);
        let placement = BackdropPlacement {
            destination: PxRect::new(
                PxPoint::new(viewfinder.x as f32, viewfinder.y as f32),
                PxSize::new(viewfinder.width as f32, viewfinder.height as f32),
            ),
        };
        let frame = UiFrame {
            draw_list: list,
            logical_size: logical,
            surface_transform: SurfaceTransform::Identity,
            backdrop: BackdropInput::Live {
                source: &source,
                placement,
            },
            live_images: ui_render_vulkan::LiveImages::none(),
        };
        let mut compositor =
            ImageCompositor::new(&self.context, self.target.format(), FRAME_SLOTS).expect("композитор");
        let mut outcome = None;
        let (renderer, target, context) = (&mut self.renderer, &self.target, &self.context);
        run_one_shot_blocking(context, |command_buffer| {
            target.record_prepare_for_render(
                command_buffer,
                ColorTargetPreviousUse::NotUsedYet,
                ContentOnPrepare::Discard,
            )?;
            compositor.record(
                command_buffer,
                PatternTarget {
                    view: target.sampled_view().view,
                    extent: target.extent(),
                },
                &CompositeRequest {
                    source: source.view(),
                    transform: SurfaceTransform::Identity,
                    destination: LogicalRect {
                        x: viewfinder.x,
                        y: viewfinder.y,
                        width: viewfinder.width,
                        height: viewfinder.height,
                    },
                    background: BackgroundColor::BLACK,
                    frame_slot: slot,
                },
            )?;
            outcome = Some(renderer.record_frame(command_buffer, slot, target.attachment(), &frame));
            Ok(())
        })
        .expect("отправка кадра");
        outcome
            .expect("замыкание выполнено")
            .expect("рендер UI принял список экрана");
        compositor.release_slot_after_fence(slot).expect("слот композитора");
        self.renderer.release_slot_after_fence(slot).expect("слот рендера");
        let pixels = read_color_target_rgba(&self.target, ColorTargetPreviousUse::Rendered).expect("readback");
        Snapshot { pixels }
    }
}
