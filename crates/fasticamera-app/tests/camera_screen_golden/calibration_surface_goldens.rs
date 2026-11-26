//! Раздел калибровки без стекла на настоящем рендере (14m, решения 263 и 270).
//!
//! Что доказывается на GPU, а не по списку рисования:
//! * кадры раздела (обе ориентации, SCALES и WHEELS, с листом меню, с карточкой пресетов, с плашкой сбоя) не строят
//!   пирамиду размытия (`PyramidOutcome::NotNeeded`) и не содержат групп стекла, а возврат в PHOTO их возвращает;
//! * пиксель панели над однородным фоном равен цвету и плотности из конфига (в допуске квантования цели);
//! * подложка плотнее панели.
//!
//! Панель прибора (`scope_panel.rs`) переводит на подложку пакет P1 прибора; пока её стекло убирается из списка перед
//! рендером (`without_scope_panel_glass`), чтобы остальной раздел проверялся уже сейчас. Когда панель прибора станет
//! поверхностью, фильтр ничего не уберёт и проверка станет строгой без правки теста.

use camera_screen::ScreenAction;
use camera_screen::modes::CaptureMode;
use look_contract::{EditState, LookProblem, LookProblemKind, PresetError};
use shell_contract::orientation::DeviceOrientation;
use ui_draw::color::Opacity;
use ui_draw::draw_list::{DrawItem, DrawList, PressState};
use ui_draw::geometry::PxSize;
use ui_draw::light::LightDirection;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::transform::Affine2;
use ui_kit::tokens::{SurfaceRole, UiKitTokens};
use ui_kit::view::{GlassShape, MemberSite, PxPerPt, surface_member};
use ui_render_vulkan::PyramidOutcome;
use ui_render_vulkan::testing::BackdropPattern;

use crate::calibration_goldens::{
    as_eye, calibration, eye_zone_window, look_facts, presets_card, render_calibration_list,
};
use crate::calibration_wheels_goldens::{golden_wheels, wheels_view};
use crate::driver::{HEIGHT_PX, PX_PER_PT, SETTLE_MS, ScreenDriver, WIDTH_PX};
use crate::render_rig::{RenderRig, ScopeImage, Snapshot, assert_no_validation_errors, gpu_context_or_skip};
use crate::{Window, with_rig};

/// Допуск на квантование 10-битной цели и округление смешивания, доля канала.
const QUANTIZATION_TOLERANCE: f32 = 0.012;
/// Серый однородного фона под экраном.
const BACKDROP_GRAY: f32 = 0.5;

/// Список без стекла панели прибора (см. заголовок модуля).
fn without_scope_panel_glass(driver: &ScreenDriver) -> DrawList {
    let layout = *driver
        .screen()
        .state()
        .calibration()
        .layout()
        .expect("раскладка раздела");
    let (left, top, right, bottom) = eye_zone_window(driver, layout.zones().scope);
    let (left, top, right, bottom) = (
        left as f32 - 3.0,
        top as f32 - 3.0,
        right as f32 + 3.0,
        bottom as f32 + 3.0,
    );
    let source = driver.list();
    let mut filtered = DrawList::new(source.light);
    filtered.items = source
        .items
        .iter()
        .filter(|item| match item {
            DrawItem::Glass(group) => !group.members.iter().all(|member| {
                member.shape_bounds_px().is_some_and(|bounds| {
                    bounds.origin.x >= left
                        && bounds.max_x() <= right
                        && bounds.origin.y >= top
                        && bounds.max_y() <= bottom
                })
            }),
            _ => true,
        })
        .cloned()
        .collect();
    filtered
}

/// Рисует список раздела и проверяет судьбу пирамиды: размытие не нужно, групп стекла нет.
fn assert_section_needs_no_blur(rig: &mut RenderRig, driver: &ScreenDriver, context: &str) {
    let list = without_scope_panel_glass(driver);
    render_calibration_list(rig, &list, ScopeImage::Absent);
    let report = rig.last_report();
    assert_eq!(
        report.pyramid,
        PyramidOutcome::NotNeeded,
        "{context}: пирамида размытия не нужна кадру раздела"
    );
    assert_eq!(report.glass_groups, 0, "{context}: в кадре раздела нет групп стекла");
    assert!(report.shape_instances > 0, "{context}: панели нарисованы фигурами");
}

fn section_with_problem(orientation: DeviceOrientation) -> ScreenDriver {
    let mut driver = calibration(orientation);
    let problem = LookProblem {
        serial: 1,
        kind: LookProblemKind::Rejected(PresetError::NameTaken),
    };
    driver.act(ScreenAction::LookFactsChanged(Box::new(look_facts(
        as_eye(),
        EditState::Clean,
        Some(problem),
    ))));
    driver.run(400);
    driver
}

fn section_with_menu(orientation: DeviceOrientation) -> ScreenDriver {
    let mut driver = calibration(orientation);
    driver.act(ScreenAction::ControlsMenuOpenRequested);
    driver.run(SETTLE_MS);
    driver
}

#[test]
fn section_frames_need_no_pyramid_and_photo_brings_it_back() {
    with_rig(|rig| {
        for orientation in [
            DeviceOrientation::Portrait,
            DeviceOrientation::LandscapeLeft,
            DeviceOrientation::LandscapeRight,
        ] {
            assert_section_needs_no_blur(rig, &calibration(orientation), &format!("{orientation:?} SCALES"));
            assert_section_needs_no_blur(
                rig,
                &wheels_view(orientation, golden_wheels()),
                &format!("{orientation:?} WHEELS"),
            );
            assert_section_needs_no_blur(
                rig,
                &section_with_menu(orientation),
                &format!("{orientation:?} лист меню"),
            );
            assert_section_needs_no_blur(
                rig,
                &section_with_problem(orientation),
                &format!("{orientation:?} плашка сбоя"),
            );
        }
        assert_section_needs_no_blur(rig, &presets_card(), "карточка пресетов");

        // Возврат в PHOTO: стекло и пирамида снова есть.
        let mut driver = calibration(DeviceOrientation::Portrait);
        driver.act(ScreenAction::ModeSelected(CaptureMode::Photo));
        driver.run(SETTLE_MS);
        crate::render(rig, &driver);
        let report = rig.last_report();
        assert!(report.glass_groups > 0, "в PHOTO стекло есть: {}", report.glass_groups);
        assert_ne!(report.pyramid, PyramidOutcome::NotNeeded, "в PHOTO пирамида строится");
    });
}

/// Ожидаемый канал поверхности: смесь фона и цвета заливки с плотностью `density`.
fn blended(background: f32, fill: f32, density: f32) -> f32 {
    background * (1.0 - density) + fill * density
}

/// Доля пикселей окна, в которых панель равна ожидаемой смеси «фон + заливка × плотность» в допуске квантования.
fn share_matching_surface(on_surface: &Snapshot, bare: &Snapshot, window: Window, density: f32) -> f32 {
    let fill = UiKitTokens::default().surface.fill_color;
    let fill = [fill.r, fill.g, fill.b];
    let (x0, y0, x1, y1) = window;
    let (mut matching, mut total) = (0_u32, 0_u32);
    for y in y0..y1 {
        for x in x0..x1 {
            let (actual, background) = (on_surface.at(x, y), bare.at(x, y));
            let fits =
                (0..3).all(|c| (actual[c] - blended(background[c], fill[c], density)).abs() <= QUANTIZATION_TOLERANCE);
            matching += u32::from(fits);
            total += 1;
        }
    }
    matching as f32 / total.max(1) as f32
}

#[test]
fn panel_pixel_over_a_uniform_backdrop_is_the_configured_colour_and_density() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut rig = RenderRig::with_backdrop(&context, BackdropPattern::Uniform([BACKDROP_GRAY; 3]));
    let tokens = UiKitTokens::default();
    let driver = calibration(DeviceOrientation::Portrait);
    let empty = DrawList::new(LightDirection::from_angle_degrees(45.0));
    let bare = render_calibration_list(&mut rig, &empty, ScopeImage::Absent);
    let on_surface = render_calibration_list(&mut rig, &without_scope_panel_glass(&driver), ScopeImage::Absent);

    // Фон действительно однородный (иначе «известный фон» — пустые слова).
    let corner = bare.at(WIDTH_PX - 8, 8);
    assert!(
        (0..3).all(|c| (corner[c] - bare.at(WIDTH_PX / 2, HEIGHT_PX / 2)[c]).abs() < 1e-3),
        "фон под экраном однородный: {corner:?}"
    );

    // Колонка ручек: внутренняя часть без каймы (скругление и тень) — подписи и риски занимают меньшую часть окна.
    let knobs = eye_zone_window(
        &driver,
        driver.screen().state().calibration().layout().unwrap().zones().knobs,
    );
    let inner = (knobs.0 + 24, knobs.1 + 24, knobs.2 - 24, knobs.3 - 24);
    let share = share_matching_surface(&on_surface, &bare, inner, tokens.surface.panel_density);
    assert!(
        share > 0.6,
        "пиксели колонки ручек = фон·(1−{d}) + заливка·{d}: совпала доля {share}",
        d = tokens.surface.panel_density
    );
    // И плотность не чужая: с плотностью подложки совпало бы заметно меньше пикселей.
    let wrong = share_matching_surface(&on_surface, &bare, inner, tokens.surface.backing_density);
    assert!(
        wrong < share * 0.2,
        "плотность панели отличима от плотности подложки: {wrong} против {share}"
    );
    drop(rig);
    assert_no_validation_errors(&context);
}

#[test]
fn backing_is_denser_than_panel_on_the_real_renderer() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut rig = RenderRig::with_backdrop(&context, BackdropPattern::Uniform([BACKDROP_GRAY; 3]));
    let tokens = UiKitTokens::default();
    let scale = PxPerPt::new(PX_PER_PT).expect("масштаб");
    // Две одинаковые плашки рядом: панель слева, подложка справа.
    let mut list = DrawList::new(LightDirection::from_angle_degrees(45.0));
    let plate = |center_x: f32, role| {
        let placement = NodePlacementPx {
            size: PxSize::new(300.0, 300.0),
            transform: Affine2::translate(center_x, 600.0),
            opacity: 1.0,
        };
        let site = MemberSite {
            shape: GlassShape::RoundedRect { corner_radius_pt: 10.0 },
            placement: &placement,
            press: PressState::RELEASED,
        };
        let (item, _host) = surface_member(&tokens, role, site, scale, 1.0, Opacity::OPAQUE).expect("поверхность");
        DrawItem::Shape(item)
    };
    list.items.push(plate(300.0, SurfaceRole::Panel));
    list.items.push(plate(700.0, SurfaceRole::Backing));
    let empty = DrawList::new(LightDirection::from_angle_degrees(45.0));
    let bare = render_calibration_list(&mut rig, &empty, ScopeImage::Absent);
    let snapshot = render_calibration_list(&mut rig, &list, ScopeImage::Absent);
    let report = rig.last_report();
    assert_eq!((report.pyramid, report.glass_groups), (PyramidOutcome::NotNeeded, 0));
    let panel_window: Window = (250, 550, 350, 650);
    let backing_window: Window = (650, 550, 750, 650);
    assert!(
        share_matching_surface(&snapshot, &bare, panel_window, tokens.surface.panel_density) > 0.99,
        "плашка-панель = конфиг (цвет и 0,70)"
    );
    assert!(
        share_matching_surface(&snapshot, &bare, backing_window, tokens.surface.backing_density) > 0.99,
        "плашка-подложка = конфиг (цвет и 0,88)"
    );
    // Подложка темнее панели над светлым фоном: фон просвечивает меньше.
    assert!(snapshot.luma(700, 600) < snapshot.luma(300, 600) - 0.02);
    drop(rig);
    assert_no_validation_errors(&context);
}
