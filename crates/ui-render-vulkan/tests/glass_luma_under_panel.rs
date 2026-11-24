//! Средняя яркость фона мерится ПОД самой панелью (решение 220, сессия 16a), а не вокруг неё.
//!
//! Фон: яркое «окно» ровно под панелью, снаружи тёмно. Панель с огромным запасом (тень радиусом 416 px, прозрачная)
//! имеет раздутые границы: при старом определении 4 из 5 точек замера лежали бы вне окна (в темноте), средняя
//! яркость вышла бы ≈ 0,2 и затемнение (включается выше 0,3) не сработало бы. Под панелью яркость ≈ 0,95:
//! затемнение полное. Тест сверяет GPU с CPU-эталоном при средней яркости, посчитанной по ЯВНОМУ прямоугольнику
//! панели (не через код продукта), и требует, чтобы картинка заметно отличалась от той, что дала бы старая область.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod support;

use ash::vk;
use gpu_vulkan::color_target::ColorTargetFormat;
use gpu_vulkan::test_pattern::SurfaceTransform;
use support::mirror::{glass_pixel, over};
use support::{
    CLEAR, Rig, assert_no_validation_errors, empty_list, full_screen_placement, glass_group, gpu_context_or_skip,
    lens_only_style, mean_luma_over_rect, pixel,
};
use ui_draw::color::DisplayColor;
use ui_draw::draw_list::{DrawItem, GlassGroupItem};
use ui_draw::geometry::{PxPoint, PxRect, PxSize};
use ui_draw::light::LightDirection;
use ui_draw::shape::{CornerSpec, ShapeGeometry};
use ui_render_vulkan::BackdropInput;
use ui_render_vulkan::testing::{BackdropPattern, SyntheticBackdrop, WINDOW_INSIDE_GRAY, WINDOW_OUTSIDE_GRAY};

const WIDTH: u32 = 1024;
const HEIGHT: u32 = 768;
/// Панель: 512 x 384 по центру экрана, границы кратны 64 (размеру текселя самого мелкого уровня пирамиды).
const PANEL: (f32, f32, f32, f32) = (256.0, 192.0, 768.0, 576.0);
/// Запас границ: тень радиусом 416 px (альфа 0: рисовать не будет, но раздувает границы группы).
const SHADOW_RADIUS_PX: f32 = 416.0;
/// Допуск на канал, как в `glass_parity`.
const TOLERANCE: f32 = 0.01;
/// Отступ от краёв панели при сравнении: у края работают блик/линза/размытие слоёв, не относящиеся к проверке.
const INSET_PX: u32 = 80;

fn panel_rect() -> PxRect {
    PxRect::new(
        PxPoint::new(PANEL.0, PANEL.1),
        PxSize::new(PANEL.2 - PANEL.0, PANEL.3 - PANEL.1),
    )
}

fn window_color(point: PxPoint) -> DisplayColor {
    let inside = point.x >= PANEL.0 && point.x < PANEL.2 && point.y >= PANEL.1 && point.y < PANEL.3;
    let gray = if inside {
        WINDOW_INSIDE_GRAY
    } else {
        WINDOW_OUTSIDE_GRAY
    };
    DisplayColor::rgb(gray, gray, gray)
}

/// Панель с затемнением по яркости фона (порог 0,3..0,6, потолок 0,6) и без остальных материалов.
fn dimming_panel() -> GlassGroupItem {
    let mut style = lens_only_style();
    style.dimming.max_alpha = 0.6;
    style.dimming.luminance_low = 0.3;
    style.dimming.luminance_high = 0.6;
    let geometry = ShapeGeometry::RoundedRect {
        size: PxSize::new(PANEL.2 - PANEL.0, PANEL.3 - PANEL.1),
        corner: CornerSpec::continuous(24.0),
    };
    let mut group = glass_group(&style, &[(geometry, PxPoint::new(512.0, 384.0))], 0.0);
    group.params.shadow.radius_px = SHADOW_RADIUS_PX;
    group.params.shadow.alpha = 0.0;
    group.params.shadow.offset_px = 0.0;
    group
}

#[test]
fn brightness_is_measured_under_the_panel_not_around_it() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let group = dimming_panel();
    // Контракт области: ровно панель, при этом границы элемента раздуты (иначе тест ничего не доказывает).
    assert_eq!(group.adaptation_rect_px().unwrap(), panel_rect());
    let inflated = group.bounds_px().unwrap();
    assert!(inflated.size.width > panel_rect().size.width + 2.0 * 400.0);

    let backdrop = SyntheticBackdrop::new(
        &context,
        vk::Extent2D {
            width: WIDTH,
            height: HEIGHT,
        },
        BackdropPattern::BrightWindow {
            left: PANEL.0,
            top: PANEL.1,
            right: PANEL.2,
            bottom: PANEL.3,
        },
    )
    .unwrap();
    let source = backdrop.source(1);
    let mut rig = Rig::new(
        &context,
        (WIDTH, HEIGHT),
        SurfaceTransform::Identity,
        ColorTargetFormat::Rgba16Float,
    );
    let light = LightDirection::from_angle_degrees(30.0);
    let mut list = empty_list();
    list.light = light;
    list.items.push(DrawItem::Glass(group.clone()));
    let input = BackdropInput::Live {
        source: &source,
        placement: full_screen_placement(WIDTH as f32, HEIGHT as f32),
    };
    let (_, pixels) = rig.render(&list, input);

    // Средняя яркость: под панелью (явный прямоугольник) и по прежнему раздутому (для контраста).
    let under_panel = mean_luma_over_rect(&panel_rect(), window_color);
    let around_panel = mean_luma_over_rect(&inflated, window_color);
    assert!(under_panel > 0.9, "под панелью яркий фон: {under_panel}");
    assert!(
        around_panel < 0.3,
        "по старой области фон выглядел бы тёмным (ниже порога затемнения): {around_panel}"
    );

    let (mut worst, mut worst_at, mut max_old_gap) = (0.0_f32, (0, 0), 0.0_f32);
    let (x_from, x_to) = (PANEL.0 as u32 + INSET_PX, PANEL.2 as u32 - INSET_PX);
    let (y_from, y_to) = (PANEL.1 as u32 + INSET_PX, PANEL.3 as u32 - INSET_PX);
    for y in (y_from..y_to).step_by(5) {
        for x in (x_from..x_to).step_by(5) {
            let point = PxPoint::new(x as f32 + 0.5, y as f32 + 0.5);
            let expected = over(CLEAR, glass_pixel(&group, light, point, under_panel, window_color));
            let old_style = over(CLEAR, glass_pixel(&group, light, point, around_panel, window_color));
            let actual = pixel(&pixels, WIDTH, x, y);
            for channel in 0..3 {
                let difference = (actual[channel] - expected[channel]).abs();
                if difference > worst {
                    worst = difference;
                    worst_at = (x, y);
                }
                max_old_gap = max_old_gap.max((actual[channel] - old_style[channel]).abs());
            }
        }
    }
    println!("яркость под панелью: расхождение с эталоном {worst} в {worst_at:?}; от старой области {max_old_gap}");
    assert!(worst <= TOLERANCE, "расхождение с эталоном {worst} в {worst_at:?}");
    assert!(
        max_old_gap > 0.2,
        "картинка должна заметно отличаться от измеренной по старой области: {max_old_gap}"
    );
    // Объекты стенда освобождаем до сверки живых объектов.
    drop(rig);
    drop(source);
    drop(backdrop);
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}
