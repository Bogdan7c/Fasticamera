//! Экран PHOTO без стекла на настоящем рендере (14p, решения 297–300).
//!
//! Что доказывается на GPU, а не по списку рисования:
//! * кадры PHOTO (покой, шторка, карточки, 16:9, альбом, фокус и блокировка, зум, отсчёт, «камера недоступна»,
//!   плашка сбоя) не строят пирамиду размытия (`PyramidOutcome::NotNeeded`) и не содержат групп стекла;
//!   единственное исключение — завеса перехода камер, пока рендер держит кадр (решение 299);
//! * пиксель панели PHOTO над однородной картинкой равен цвету и плотности из конфига, над чёрной полосой 4:3 —
//!   заливке с той же плотностью, а у края силуэта есть светлый ободок, который за силуэт не выходит.

use camera_screen::settings::AspectRatio;
use ui_kit::tokens::UiKitTokens;
use ui_render_vulkan::PyramidOutcome;
use ui_render_vulkan::testing::BackdropPattern;

use crate::calibration_goldens::problem_banner;
use crate::calibration_surface_goldens::{BACKDROP_GRAY, QUANTIZATION_TOLERANCE, share_matching_surface};
use crate::driver::{PX_PER_PT, ScreenDriver};
use crate::render_rig::{RenderRig, assert_no_validation_errors, gpu_context_or_skip};
use crate::{Window, render, states, with_rig, zoom_states};

/// Состояние экрана с именем для сообщения: строится по требованию (`fn`), а не заранее.
type NamedState = (&'static str, fn() -> ScreenDriver);

/// Рисует кадр PHOTO и проверяет судьбу пирамиды: размытие не нужно, групп стекла нет, панели — фигуры.
fn assert_photo_needs_no_blur(rig: &mut RenderRig, driver: &ScreenDriver, context: &str) {
    render(rig, driver);
    let report = rig.last_report();
    assert_eq!(
        report.pyramid,
        PyramidOutcome::NotNeeded,
        "{context}: пирамида размытия не нужна кадру PHOTO"
    );
    assert_eq!(report.glass_groups, 0, "{context}: в кадре PHOTO нет групп стекла");
    assert!(report.shape_instances > 0, "{context}: панели нарисованы фигурами");
}

#[test]
fn photo_frames_need_no_pyramid_and_only_the_held_frame_veil_is_glass() {
    with_rig(|rig| {
        // Состояния строятся по одному (экран большой, массив готовых экранов переполнил бы стек потока теста).
        let cases: [NamedState; 14] = [
            ("покой", states::rest),
            ("шторка Controls", states::controls_menu),
            ("карточка Exposure", || states::exposure_panel(1.0)),
            ("карточка Timer", states::timer_submenu),
            ("16:9", || states::with_aspect(AspectRatio::SixteenByNine)),
            ("альбом влево", states::landscape_left),
            ("рамка фокуса", || states::focus_frame_after_tap().0),
            ("AE/AF LOCK", || states::ae_af_lock().0),
            ("отсчёт таймера", states::timer_countdown),
            ("камера недоступна", states::camera_unavailable),
            ("лента зума", zoom_states::strip_with_ultra_wide_selected),
            ("колесо зума", zoom_states::wheel_open_at_one_point_seven),
            ("фронтальная камера", zoom_states::front_camera_without_strip),
            ("плашка сбоя", problem_banner),
        ];
        for (context, build) in cases {
            let driver = Box::new(build());
            assert_photo_needs_no_blur(rig, &driver, context);
        }
        // Завеса перехода (решение 299): пока рендер держит кадр, стекло одно — завеса — и пирамида строится.
        let veiled: [NamedState; 2] = [
            ("смена объектива", zoom_states::held_frame_while_switching_to_ultra_wide),
            ("переворот камеры", zoom_states::flip_card_mid_rotation),
        ];
        for (context, build) in veiled {
            let driver = Box::new(build());
            render(rig, &driver);
            let report = rig.last_report();
            assert_eq!(report.glass_groups, 1, "{context}: стекло только у завесы");
            assert_ne!(
                report.pyramid,
                PyramidOutcome::NotNeeded,
                "{context}: завесе нужно размытие"
            );
        }
    });
}

#[test]
fn photo_panel_pixels_are_the_configured_density_over_the_picture_and_over_black_with_a_light_edge() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut rig = RenderRig::with_backdrop(&context, BackdropPattern::Uniform([BACKDROP_GRAY; 3]));
    let tokens = UiKitTokens::default();
    let density = tokens.surface.panel_density;
    let scale = f64::from(PX_PER_PT);
    let to_px = |pt: f64| (pt * scale).round() as u32;

    // Фон «как без интерфейса»: тот же кадр (картинка в окне 4:3, чёрная маска вне его), но без панелей. Пустой
    // список не годится — маску вне кадра рисует сам экран, а у рамки фокуса и ленты свои фигуры.
    let rest = states::rest();
    let bare = render(&mut rig, &rest);

    // Шторка над картинкой: окно между первой и второй колонками пунктов (x 135…175 pt), выше кругов пунктов и
    // ниже «ручки» (y 530…552 pt) — здесь только поверхность. Кадр 4:3 тянется до 772,7 pt, окно — над картинкой.
    let sheet = render(&mut rig, &states::controls_menu());
    let over_picture: Window = (to_px(135.0), to_px(530.0), to_px(175.0), to_px(552.0));
    let share = share_matching_surface(&sheet, &bare, over_picture, density);
    assert!(
        share > 0.95,
        "пиксели шторки над картинкой = фон·(1−{density}) + заливка·{density}: доля {share}"
    );
    let as_backing = share_matching_surface(&sheet, &bare, over_picture, tokens.surface.backing_density);
    assert!(as_backing < 0.05, "плотность — панельная, не подложки: {as_backing}");

    // Карточка Exposure лежит на чёрной полосе под кадром 4:3: внутри — заливка с плотностью панели поверх чёрного.
    let driver = states::exposure_panel(0.0);
    let card = render(&mut rig, &driver);
    let (x0, y0, _x1, y1) = driver.panel_rect_px().expect("поверхность карточки");
    let mid_y = ((y0 + y1) / 2.0) as u32;
    let fill = tokens.surface.fill_color;
    let inner = card.at(x0 as u32 + 30, mid_y);
    assert!(
        (inner[0] - fill.r * density).abs() <= QUANTIZATION_TOLERANCE
            && (inner[2] - fill.b * density).abs() <= QUANTIZATION_TOLERANCE,
        "внутри карточки над чёрным = заливка × {density}: {inner:?}"
    );
    // Ободок (решение 297): последний 1 pt внутри силуэта светлее заливки, за силуэтом — чёрная полоса.
    let edge_px = (f64::from(tokens.surface.edge.width_pt) * scale) as f32;
    let band = card.at((x0 + edge_px * 0.5) as u32, mid_y);
    assert!(
        band[0] > inner[0] + 0.1,
        "линия ободка светлее заливки: {band:?} против {inner:?}"
    );
    let outside = card.at((x0 - 2.0) as u32, mid_y);
    assert!(
        outside.iter().take(3).all(|channel| *channel < 0.01),
        "за силуэтом карточки — чёрная полоса: {outside:?}"
    );
    drop(rig);
    assert_no_validation_errors(&context);
}
