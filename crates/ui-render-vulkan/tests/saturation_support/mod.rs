//! Помощники тестов адаптера кадра UI под стенд насыщения: сцена «типичный кадр камеры» и эталонный кадр,
//! записанный ОБЫЧНЫМ путём (одноразовая блокирующая отправка, без стенда) теми же вызовами на тех же входах.
//!
//! Сцена повторяет `tests/timing.rs` (1080 × 2412, 8 стёкол, подписи, иконки). Тестовые бинарники Rust
//! изолированы друг от друга, а подключить `timing.rs` как модуль значило бы притащить его тест в этот бинарь, поэтому
//! построение сцены продублировано здесь: `timing.rs` по заданию не меняется.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "общий модуль подключается разными тестовыми файлами; в тестах падение — способ сообщить о провале"
)]

use ash::vk;
use gpu_vulkan::color_target::{ColorTarget, ColorTargetDesc, ColorTargetPreviousUse, ContentOnPrepare};
use gpu_vulkan::compositor::{BackgroundColor, CompositeRequest, ImageCompositor, LogicalRect};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::kit_test_support::{read_color_target_rgba, run_one_shot_blocking};
use gpu_vulkan::raster::{AttachmentLoad, RasterPass, record_between_color_passes};
use gpu_vulkan::test_pattern::{PatternTarget, SurfaceTransform};
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{DrawItem, DrawList, GlassGroupItem, GlassMember, IconItem, PressState, TextItem};
use ui_draw::geometry::{PxPoint, PxRect, PxSize};
use ui_draw::glass::GlassStyle;
use ui_draw::icons::IconId;
use ui_draw::shape::{CornerSpec, ShapeGeometry};
use ui_draw::text::{Figures, FontFace, TextStyle, layout_line};
use ui_draw::transform::Affine2;
use ui_render_vulkan::saturation::{DEFAULT_TARGET_EXTENT, DEFAULT_TARGET_FORMAT, FrameComposition, SaturationScene};
use ui_render_vulkan::testing::{BackdropPattern, SyntheticBackdrop};
use ui_render_vulkan::{BackdropInput, BackdropPlacement, LiveImages, UiFrame, UiRenderer, UiRendererConfig};

use ui_kit::tokens::{GlassRole, GlassTokens};

use crate::support::{empty_list, glass_group};

/// Пикселей на pt при ширине экрана 440 pt (как у `timing.rs`).
const PX_PER_PT: f32 = 1080.0 / 440.0;
/// Размер картинки «выхода ISP» (≈ продуктовый видоискатель).
pub const BACKDROP_EXTENT: vk::Extent2D = vk::Extent2D {
    width: 2048,
    height: 1536,
};
/// Где компоновщик кладёт картинку 4:3 на экран 1080 × 2412: по центру, во всю ширину.
pub const VIEWFINDER: LogicalRect = LogicalRect {
    x: 0,
    y: 486,
    width: 1080,
    height: 1440,
};
/// Пиксель внутри видоискателя, до которого не дотягивается ни один элемент UI (проверка «компоновщик записан / нет»).
pub const FREE_VIEWFINDER_PIXEL: (usize, usize) = (540, 1100);

/// Типичный кадр камеры: 8 стёкол, 6 подписей, 6 иконок.
pub fn typical_draw_list() -> DrawList {
    let mut list = empty_list();
    let style = GlassStyle::default();
    let button = 48.0 * PX_PER_PT;
    for index in 0..4 {
        let center = PxPoint::new(180.0 + index as f32 * 240.0, 2150.0);
        list.items.push(DrawItem::Glass(glass_group(
            &style,
            &[(ShapeGeometry::Circle { diameter: button }, center)],
            0.0,
        )));
    }
    list.items.push(DrawItem::Glass(glass_group(
        &style,
        &[(
            ShapeGeometry::Capsule {
                size: PxSize::new(560.0, 120.0),
            },
            PxPoint::new(540.0, 1900.0),
        )],
        0.0,
    )));
    for index in 0..3 {
        list.items.push(DrawItem::Glass(glass_group(
            &style,
            &[(
                ShapeGeometry::Circle { diameter: button },
                PxPoint::new(200.0 + index as f32 * 340.0, 200.0),
            )],
            0.0,
        )));
    }
    let label = TextStyle {
        face: FontFace::Semibold,
        size_px: 40.0,
        tracking_em: 0.0,
        figures: Figures::Tabular,
    };
    for index in 0..6 {
        list.items.push(DrawItem::Text(TextItem {
            run: layout_line("PHOTO  VIDEO  PORTRAIT 1.0x", &label).unwrap(),
            transform: Affine2::translate(60.0, 1700.0 + index as f32 * 20.0),
            color: DisplayColor::WHITE,
            opacity: Opacity::OPAQUE,
        }));
    }
    for index in 0..6 {
        list.items.push(DrawItem::Icon(IconItem {
            icon: IconId::Grid,
            transform: Affine2::translate(100.0 + index as f32 * 150.0, 400.0),
            size_px: PxSize::new(64.0, 64.0),
            color: DisplayColor::WHITE,
            opacity: Opacity::OPAQUE,
        }));
    }
    list
}

/// Сцена для адаптера: типичный кадр камеры над синтетическим «фото».
pub fn typical_scene(context: &GpuContext) -> SaturationScene {
    SaturationScene {
        draw_list: typical_draw_list(),
        live_images: Vec::new(),
        viewfinder: VIEWFINDER,
        backdrop: Box::new(SyntheticBackdrop::new(context, BACKDROP_EXTENT, BackdropPattern::Photo).unwrap()),
    }
}

/// Эталон: кадр, записанный обычным путём вне стенда (один слот, блокирующая отправка) теми же вызовами на тех
/// же входах. Возвращает пиксели цели сверху вниз.
pub fn render_reference(context: &GpuContext, composition: FrameComposition) -> Vec<[f32; 4]> {
    let backdrop = SyntheticBackdrop::new(context, BACKDROP_EXTENT, BackdropPattern::Photo).unwrap();
    let source = backdrop.source(0);
    let format = DEFAULT_TARGET_FORMAT;
    let target = ColorTarget::new(
        context,
        &ColorTargetDesc {
            extent: DEFAULT_TARGET_EXTENT,
            format,
            label: "saturation-reference",
        },
    )
    .unwrap();
    let mut compositor = ImageCompositor::new(context, format.vk_format(), 1).unwrap();
    let mut renderer = UiRenderer::new(context, &UiRendererConfig::new(format.vk_format(), 1)).unwrap();
    let list = typical_draw_list();
    let frame = UiFrame {
        draw_list: &list,
        logical_size: PxSize::new(DEFAULT_TARGET_EXTENT.width as f32, DEFAULT_TARGET_EXTENT.height as f32),
        surface_transform: SurfaceTransform::Identity,
        backdrop: BackdropInput::Live {
            source: &source,
            placement: BackdropPlacement {
                destination: PxRect::new(
                    PxPoint::new(VIEWFINDER.x as f32, VIEWFINDER.y as f32),
                    PxSize::new(VIEWFINDER.width as f32, VIEWFINDER.height as f32),
                ),
            },
        },
        live_images: LiveImages::none(),
    };
    run_one_shot_blocking(context, |command_buffer| {
        target.record_prepare_for_render(
            command_buffer,
            ColorTargetPreviousUse::NotUsedYet,
            ContentOnPrepare::Discard,
        )?;
        if composition.includes_compositor() {
            compositor.record(
                command_buffer,
                PatternTarget {
                    view: target.sampled_view().view,
                    extent: target.extent(),
                },
                &CompositeRequest {
                    source: source.view(),
                    transform: SurfaceTransform::Identity,
                    destination: VIEWFINDER,
                    background: BackgroundColor::BLACK,
                    frame_slot: 0,
                },
            )?;
        } else {
            RasterPass::begin(
                context,
                command_buffer,
                target.attachment(),
                AttachmentLoad::Clear([0.0, 0.0, 0.0, 1.0]),
            )?
            .end();
        }
        if composition.includes_ui() {
            record_between_color_passes(context, command_buffer);
            renderer
                .record_frame(command_buffer, 0, target.attachment(), &frame)
                .expect("рендер UI принял кадр эталона");
        }
        Ok(())
    })
    .unwrap();
    read_color_target_rgba(&target, ColorTargetPreviousUse::Rendered).unwrap()
}

/// Индекс пикселя `(x, y)` в плоском массиве readback цели по умолчанию.
pub fn pixel_index(x: usize, y: usize) -> usize {
    y * DEFAULT_TARGET_EXTENT.width as usize + x
}

/// Размер панели «ручки калибровки» в px при экране 1080 × 2412: примерно половина экрана по ширине и почти вся высота.
pub const PANEL_SIZE: PxSize = PxSize {
    width: 540.0,
    height: 1950.0,
};
/// Центр панели: прижата к правому краю экрана, по высоте почти во весь экран.
pub const PANEL_CENTER: PxPoint = PxPoint { x: 810.0, y: 1206.0 };
/// Радиус скругления панели калибровки, pt (`panel_corner_radius_pt` конфигурации экрана).
const PANEL_CORNER_RADIUS_PT: f32 = 26.0;

/// Одна стеклянная группа «панель на пол-экрана»: стиль продукта `GlassRole::Panel` (токены `ui-kit`), плотность
/// экрана 1080 px на 440 pt, как у остальных сцен. Это самая дорогая отдельная фигура экрана калибровки.
pub fn panel_group() -> GlassGroupItem {
    let tokens = GlassTokens::default();
    let style = tokens.style_for(GlassRole::Panel);
    GlassGroupItem {
        params: style.resolve(PANEL_SIZE, PX_PER_PT),
        members: vec![GlassMember {
            geometry: ShapeGeometry::RoundedRect {
                size: PANEL_SIZE,
                corner: CornerSpec::continuous(PANEL_CORNER_RADIUS_PT * PX_PER_PT),
            },
            transform: Affine2::translate(PANEL_CENTER.x, PANEL_CENTER.y),
            press: PressState::RELEASED,
        }],
        merge_radius_px: 0.0,
        effect: 1.0,
        opacity: Opacity::OPAQUE,
    }
}

/// Кадр из одной панели (без подписей и кнопок: цена панели отдельно от остального).
pub fn panel_draw_list() -> DrawList {
    let mut list = empty_list();
    list.items.push(DrawItem::Glass(panel_group()));
    list
}

/// Сцена «панель на пол-экрана» над синтетическим «фото».
pub fn panel_scene(context: &GpuContext) -> SaturationScene {
    SaturationScene {
        draw_list: panel_draw_list(),
        live_images: Vec::new(),
        viewfinder: VIEWFINDER,
        backdrop: Box::new(SyntheticBackdrop::new(context, BACKDROP_EXTENT, BackdropPattern::Photo).unwrap()),
    }
}
