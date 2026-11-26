//! Golden-кадры раздела калибровки, карточек пресетов и плашки проблемы (сессия 14b, пакет P6c).
//!
//! Раздел калибровки (портрет и альбом), карточка PRESETS меню раздела, карточка STYLES экрана PHOTO и плашка
//! проблемы доводятся до вида настоящими действиями экрана и рисуются настоящим `UiRenderer`. Как и остальные
//! golden: кроме сверки с эталоном каждый тест проверяет пиксели независимо от него.
//!
//! Прибор. Настоящего цветового прибора в стенде нет (его считает GPU-часть рендера, её ловят сквозные тесты 14h),
//! поэтому на зону прибора кладётся ДЕТЕРМИНИРОВАННАЯ ПОДСТАВНАЯ картинка — то же синтетическое «фото» (`ScopeImage::
//! StandIn`). Она доказывает, что картинка прибора лежит на своей зоне стеклянной панели, а не как выглядит прибор.

use camera_screen::modes::CaptureMode;
use camera_screen::tools::ToolId;
use camera_screen::{PanelState, ScreenAction};
use fasticamera::look_facts::{BandSwatchStyle, LookFactsCatalog};
use fasticamera::look_wheel_ring::WheelRingStyle;
use look_contract::ColorWheels;
use look_contract::{
    BandFacts, BandSwatch, BuiltinPreset, EditState, HueBandId, LookFacts, LookProblem, LookProblemKind, LookRecipe,
    LookSettings, PresetEntry, PresetError, PresetId, PresetKind, PresetName, ToneBaseModel, UserPresetId,
    WheelHueRing,
};
use render_loop::config::RenderLoopConfig;
use shell_contract::orientation::DeviceOrientation;
use ui_core::geometry::Rect;
use ui_draw::draw_list::DrawList;
use ui_draw::light::LightDirection;

use crate::driver::{HEIGHT_PX, PX_PER_PT, SETTLE_MS, ScreenDriver, WIDTH_PX};
use crate::golden_files::check_golden;
use crate::render_rig::{RenderRig, ScopeImage, Snapshot, ViewfinderRectPx};
use crate::{CARD_RIGHT_BUTTON_WINDOW, Window, card_top_px, focus_yellow, with_rig};

/// Свой пресет «Sunset» (идентификатор 7).
const SUNSET: UserPresetId = UserPresetId(7);

fn preset(id: PresetId, name: &str, kind: PresetKind) -> PresetEntry {
    PresetEntry {
        id,
        name: PresetName::new(name).expect("имя допустимо"),
        kind,
    }
}

/// Факты о виде: два встроенных и два своих пресета, три полосы; активен `active`, правки — `edits`.
/// Колёса нейтральны.
pub(crate) fn look_facts(active: PresetId, edits: EditState, problem: Option<LookProblem>) -> LookFacts {
    look_facts_with_wheels(active, edits, problem, ColorWheels::NEUTRAL)
}

/// Настоящий ободок колёс: цвета OKLab из каталога приложения по умолчанию (а не однотонная заглушка), чтобы golden
/// и пиксельные проверки видели те оттенки, что рисует продукт.
pub(crate) fn real_wheel_ring() -> WheelHueRing {
    LookFactsCatalog::from_render_config(&RenderLoopConfig::default(), &BandSwatchStyle::default())
        .with_wheel_ring_style(&WheelRingStyle::default())
        .wheel_ring()
        .clone()
}

/// Те же факты, но ТЕКУЩИЙ вид несёт колёса `wheels` (пресет остаётся нейтральным: отличие от пресета и даёт яркие «↻»).
pub(crate) fn look_facts_with_wheels(
    active: PresetId,
    edits: EditState,
    problem: Option<LookProblem>,
    wheels: ColorWheels,
) -> LookFacts {
    let preset_recipe = LookRecipe {
        base: ToneBaseModel::Agx,
        settings: LookSettings::NEUTRAL,
    };
    let recipe = LookRecipe {
        base: ToneBaseModel::Agx,
        settings: LookSettings {
            wheels,
            ..LookSettings::NEUTRAL
        },
    };
    LookFacts {
        active,
        presets: vec![
            preset(PresetId::Builtin(BuiltinPreset::Honest), "Honest", PresetKind::Builtin),
            preset(PresetId::Builtin(BuiltinPreset::AsEye), "As Eye", PresetKind::Builtin),
            preset(PresetId::User(SUNSET), "Sunset", PresetKind::User),
            preset(PresetId::User(UserPresetId(9)), "Night", PresetKind::User),
        ],
        current: recipe,
        preset: preset_recipe,
        edits,
        bands: ["red", "sky", "green"]
            .into_iter()
            .map(|id| BandFacts {
                id: HueBandId::new(id).expect("идентификатор полосы допустим"),
                swatch: BandSwatch { srgb: [200, 40, 40] },
            })
            .collect(),
        wheel_ring: real_wheel_ring(),
        problem,
    }
}

pub(crate) fn as_eye() -> PresetId {
    PresetId::Builtin(BuiltinPreset::AsEye)
}

/// Раздел калибровки: факты о виде, положение телефона, режим CALIBRATE.
pub(crate) fn calibration(orientation: DeviceOrientation) -> ScreenDriver {
    let mut driver = ScreenDriver::new();
    driver.act(ScreenAction::LookFactsChanged(Box::new(look_facts(
        as_eye(),
        EditState::Clean,
        None,
    ))));
    driver.act(ScreenAction::DeviceOrientationChanged(orientation));
    driver.act(ScreenAction::ModeSelected(CaptureMode::Calibrate));
    driver.run(SETTLE_MS);
    driver
}

/// Карточка PRESETS меню раздела: правки поверх своего пресета «Sunset» (SAVE, RENAME, DELETE и «↻» доступны).
pub(crate) fn presets_card() -> ScreenDriver {
    let mut driver = ScreenDriver::new();
    driver.act(ScreenAction::LookFactsChanged(Box::new(look_facts(
        PresetId::User(SUNSET),
        EditState::EditedOnTop,
        None,
    ))));
    driver.act(ScreenAction::ModeSelected(CaptureMode::Calibrate));
    driver.run(SETTLE_MS);
    driver.act(ScreenAction::ControlsMenuOpenRequested);
    driver.run(SETTLE_MS);
    driver
}

/// Карточка STYLES экрана PHOTO: меню Controls → пункт STYLES; `edits` решает, яркая ли «↻».
fn styles_card(edits: EditState) -> ScreenDriver {
    let mut driver = ScreenDriver::new();
    driver.act(ScreenAction::LookFactsChanged(Box::new(look_facts(
        PresetId::User(SUNSET),
        edits,
        None,
    ))));
    driver.act(ScreenAction::ControlsMenuOpenRequested);
    driver.act(ScreenAction::ControlsItemSelected(ToolId::Styles));
    driver.run(SETTLE_MS);
    driver
}

/// PHOTO с проблемой библиотеки «имя занято»; плашка уже появилась и ещё не погасла.
pub(crate) fn problem_banner() -> ScreenDriver {
    let mut driver = ScreenDriver::new();
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

/// В разделе калибровки видоискатель — весь экран (его рисует рендер), а не кадр 4:3 экрана PHOTO.
pub(crate) fn render_calibration(rig: &mut RenderRig, driver: &ScreenDriver, scope: ScopeImage) -> Snapshot {
    render_calibration_list(rig, driver.list(), scope)
}

pub(crate) fn render_calibration_list(rig: &mut RenderRig, list: &DrawList, scope: ScopeImage) -> Snapshot {
    rig.render_with_scope(
        list,
        ViewfinderRectPx {
            x: 0,
            y: 0,
            width: WIDTH_PX,
            height: HEIGHT_PX,
        },
        scope,
    )
}

/// Глазной прямоугольник раздела → окно на поверхности, px.
pub(crate) fn eye_zone_window(driver: &ScreenDriver, eye: Rect) -> Window {
    let layout = driver
        .screen()
        .state()
        .calibration()
        .layout()
        .expect("раскладка раздела при режиме CALIBRATE");
    let rect = layout.eye_rect_on_surface(eye);
    let scale = f64::from(PX_PER_PT);
    (
        (rect.min_x() * scale).round() as u32,
        (rect.min_y() * scale).round() as u32,
        (rect.max_x() * scale).round() as u32,
        (rect.max_y() * scale).round() as u32,
    )
}

/// Кайма вокруг стеклянной панели, где её линза и тень ещё меняют соседние пиксели, px.
const GLASS_HALO_PX: u32 = 24;

/// Сколько пикселей окна заметно (больше `VISIBLE_DIFFERENCE`) отличаются между снимками. Среднее здесь не годится:
/// маленькая подпись на большом окне почти не двигает среднее, а один лишний значок — уже поломка.
pub(crate) fn differing_pixels(first: &Snapshot, second: &Snapshot, window: Window) -> usize {
    let (x0, y0, x1, y1) = window;
    let mut count = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            let (a, b) = (first.at(x, y), second.at(x, y));
            if (0..3).any(|channel| (a[channel] - b[channel]).abs() > VISIBLE_DIFFERENCE) {
                count += 1;
            }
        }
    }
    count
}

/// Разница канала, с которой пиксель считается нарисованным поверх (выше шума 10-битной цели и тонкого хвоста тени стекла у края панели; подпись или значок дают на порядок больше).
pub(crate) const VISIBLE_DIFFERENCE: f32 = 0.05;

/// Один видоискатель без интерфейса: пустой список рисования поверх того же «фото» на весь экран.
pub(crate) fn bare_viewfinder(rig: &mut RenderRig) -> Snapshot {
    let empty = DrawList::new(LightDirection::from_angle_degrees(45.0));
    render_calibration_list(rig, &empty, ScopeImage::Absent)
}

/// Общие проверки раздела калибровки: чистая зона — живой видоискатель без UI; прибор лежит ровно на своей зоне;
/// выбранный режим CALIBRATE жёлтый в капсуле.
pub(crate) fn assert_calibration_zones(rig: &mut RenderRig, driver: &ScreenDriver, snapshot: &Snapshot, context: &str) {
    let layout = *driver.screen().state().calibration().layout().expect("раскладка");
    let zones = *layout.zones();
    let clean = eye_zone_window(driver, zones.clean_preview);
    let scope = eye_zone_window(driver, zones.scope);
    let knobs = eye_zone_window(driver, zones.knobs);
    // Чистая зона — кусок большого кадра (решение 203): поверх живого изображения не нарисовано ничего.
    // Окно сужено на кайму стекла соседних панелей: его линза и тень заходят за край панели на несколько пикселей.
    let bare = bare_viewfinder(rig);
    let clean_inner = (
        clean.0 + GLASS_HALO_PX,
        clean.1 + GLASS_HALO_PX,
        clean.2 - GLASS_HALO_PX,
        clean.3 - GLASS_HALO_PX,
    );
    assert_eq!(
        differing_pixels(snapshot, &bare, clean_inner),
        0,
        "{context}: ни один пиксель чистой зоны не отличается от видоискателя без интерфейса"
    );
    assert!(
        snapshot.mean_difference_in(&bare, knobs) > 0.01,
        "{context}: колонка ручек — стекло поверх видоискателя"
    );
    // Тот же экран без картинки прибора: различие только в зоне прибора.
    let without_scope = render_calibration(rig, driver, ScopeImage::Absent);
    assert!(
        snapshot.mean_difference_in(&without_scope, scope) > 0.02,
        "{context}: подставная картинка прибора видна на зоне прибора"
    );
    assert!(
        snapshot.mean_difference_in(&without_scope, clean) < 1e-6,
        "{context}: вне зоны прибора (чистая зона) картинки прибора нет"
    );
    assert!(
        snapshot.mean_difference_in(&without_scope, knobs) < 1e-6,
        "{context}: вне зоны прибора (колонка ручек) картинки прибора нет"
    );
    // Выбранный режим — жёлтый по центру капсулы (глазной центр нижнего ряда).
    let capsule_center = layout.carousel_center();
    let capsule = eye_zone_window(
        driver,
        Rect::centered_at(capsule_center, ui_core::geometry::Size::new(80.0, 24.0)),
    );
    assert!(
        snapshot.count_in(capsule, focus_yellow) > 40,
        "{context}: подпись CALIBRATE жёлтая по центру капсулы"
    );
}

#[test]
fn calibration_portrait() {
    with_rig(|rig| {
        let driver = calibration(DeviceOrientation::Portrait);
        let snapshot = render_calibration(rig, &driver, ScopeImage::StandIn);
        check_golden("calibration_portrait", &snapshot);
        assert_calibration_zones(rig, &driver, &snapshot, "портрет");
    });
}

#[test]
fn calibration_landscape_left() {
    with_rig(|rig| {
        let driver = calibration(DeviceOrientation::LandscapeLeft);
        let snapshot = render_calibration(rig, &driver, ScopeImage::StandIn);
        check_golden("calibration_landscape_left", &snapshot);
        assert_calibration_zones(rig, &driver, &snapshot, "альбом, левый бок");
    });
}

/// Окно ряда таблеток карточки: вся ширина карточки на высоте ряда вариантов (T+118 pt), ±20 pt по высоте.
fn card_chip_row_window() -> Window {
    let scale = f64::from(PX_PER_PT);
    let center_y = f64::from(card_top_px()) + 118.0 * scale;
    (
        (20.0 * scale) as u32,
        (center_y - 20.0 * scale) as u32,
        (420.0 * scale) as u32,
        (center_y + 20.0 * scale) as u32,
    )
}

#[test]
fn calibration_presets_card() {
    with_rig(|rig| {
        let driver = presets_card();
        assert_eq!(
            driver.screen().state().calibration().menu(),
            camera_screen::calibration::CalibrationMenu::Presets
        );
        let snapshot = render_calibration(rig, &driver, ScopeImage::StandIn);
        // Активный пресет «Sunset» — жёлтым в ряду имён; остальные имена белые.
        let yellow = snapshot.count_in(card_chip_row_window(), focus_yellow);
        assert!(yellow > 60, "активное имя жёлтое в ряду: {yellow} жёлтых пикселей");
        // «↻» яркая (есть правки): светлый значок на тёмном диске правой кнопки.
        let reset = snapshot.count_in(CARD_RIGHT_BUTTON_WINDOW, |pixel| {
            pixel.iter().all(|channel| *channel > 0.6)
        });
        assert!(reset > 25, "«↻» карточки PRESETS яркая: {reset}");
        // Карточка стоит у низа экрана на своей высоте: над её верхом пиксели те же, что без меню.
        let closed = calibration(DeviceOrientation::Portrait);
        let closed_snapshot = render_calibration(rig, &closed, ScopeImage::StandIn);
        let above_card: Window = (40, 200, WIDTH_PX - 40, 700);
        assert!(
            snapshot.mean_difference_in(&closed_snapshot, above_card) < 0.05,
            "верх экрана почти не меняется от меню"
        );
        check_golden("calibration_presets_card", &snapshot);
    });
}

#[test]
fn photo_styles_card() {
    with_rig(|rig| {
        let driver = styles_card(EditState::EditedOnTop);
        assert_eq!(driver.screen().state().panel(), PanelState::Submenu(ToolId::Styles));
        let snapshot = crate::render(rig, &driver);
        let yellow = snapshot.count_in(card_chip_row_window(), focus_yellow);
        assert!(yellow > 60, "активное имя жёлтое в ряду: {yellow} жёлтых пикселей");
        // «↻» с правками — яркая, без правок — тусклая (44 %): разница видна в окне правой кнопки.
        let clean = crate::render(rig, &styles_card(EditState::Clean));
        assert!(
            snapshot.mean_difference_in(&clean, CARD_RIGHT_BUTTON_WINDOW) > 0.01,
            "«↻» с правками ярче, чем без них"
        );
        check_golden("photo_styles_card", &snapshot);
    });
}

#[test]
fn problem_banner_in_photo() {
    with_rig(|rig| {
        let driver = problem_banner();
        let snapshot = crate::render(rig, &driver);
        let rest = crate::render(rig, &ScreenDriver::new());
        // Плашка стоит по центру кадра у его верха (там же, где AE/AF LOCK): ширина по тексту, высота токена.
        let scale = f64::from(PX_PER_PT);
        let frame_top = driver
            .screen()
            .state()
            .layout()
            .expect("раскладка")
            .frame_rect(camera_screen::settings::AspectRatio::FourByThree)
            .min_y();
        let window: Window = (
            (150.0 * scale) as u32,
            ((frame_top + 4.0) * scale) as u32,
            (290.0 * scale) as u32,
            ((frame_top + 48.0) * scale) as u32,
        );
        assert!(
            snapshot.mean_difference_in(&rest, window) > 0.03,
            "плашка проблемы нарисована у верха кадра"
        );
        check_golden("problem_banner_photo", &snapshot);
    });
}
