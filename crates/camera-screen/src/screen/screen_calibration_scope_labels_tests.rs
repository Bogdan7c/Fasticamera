//! Подписи прибора на экране калибровки (решения 326, 327): буквы целей вектороскопа рисуются поверх картинки в точках,
//! которые прислал рендер, и ТОЛЬКО для текущего запроса прибора (свайп, оси и зум не оставляют чужих букв).

use scope_contract::{
    LabelPoint, ScopeKind, ScopeLabel, ScopeLabels, ScopeLabelsState, ScopeMark, ScopeRequest, TargetColor,
    VectorscopeAxes, VectorscopeZoom,
};
use shell_contract::orientation::DeviceOrientation;
use ui_core::geometry::Point;
use ui_kit::tokens::TextRoleKind;

use super::screen_calibration_tests::calibrating;
use super::test_rig::*;
use crate::action::ScreenAction;
use crate::look_test_support::look_facts;

fn label(color: TargetColor, x: f32, y: f32) -> ScopeLabel {
    ScopeLabel {
        mark: ScopeMark::VectorscopeTarget(color),
        at: LabelPoint::try_new(x, y).expect("точка в картинке"),
    }
}

fn published(request: ScopeRequest, labels: Vec<ScopeLabel>) -> ScreenAction {
    ScreenAction::ScopeLabelsChanged(ScopeLabelsState::Published(ScopeLabels { request, labels }))
}

fn desired_request(rig: &Rig) -> ScopeRequest {
    rig.screen
        .state()
        .calibration()
        .desired_scope_request()
        .expect("в разделе есть раскладка, значит есть и желаемый запрос")
}

/// Раздел на странице вектороскопа с обычным масштабом и осями Y'CbCr.
fn vectorscope_page(orientation: DeviceOrientation) -> Rig {
    let mut rig = calibrating(look_facts(), orientation);
    rig.act(ScreenAction::ScopeKindSelected(ScopeKind::Vectorscope));
    rig.act(ScreenAction::VectorscopeAxesSelected(VectorscopeAxes::YCbCrBt709));
    rig.act(ScreenAction::VectorscopeZoomSelected(VectorscopeZoom::Normal));
    rig.run(300);
    rig
}

/// Сколько букв «R», «MG», «B», «CY», «G», «YL» нарисовано (каждая роли Caption).
fn drawn_letters(rig: &mut Rig) -> usize {
    let list = rig.frame().clone();
    ["R", "MG", "B", "CY", "G", "YL"]
        .iter()
        .map(|text| text_count_of(&list, text, TextRoleKind::Caption))
        .sum()
}

/// Центр нарисованной подписи `text` в глазных осях раздела (pt).
fn drawn_centre(rig: &mut Rig, orientation_name: &str, text: &str) -> Point {
    let layout = *rig.screen.state().calibration().layout().expect("раскладка");
    let list = rig.frame().clone();
    let runs = texts_of(&list, text, TextRoleKind::Caption);
    assert_eq!(runs.len(), 1, "{orientation_name}: буква «{text}» нарисована один раз");
    let item = &runs[0];
    let baseline = layout.surface_point_in_eye(Point::new(
        f64::from(item.transform.tx) / f64::from(px(1.0)),
        f64::from(item.transform.ty) / f64::from(px(1.0)),
    ));
    // Начало текста — левый конец базовой линии: центр правее на половину ширины строки и выше на половину высоты букв.
    Point::new(
        baseline.x + f64::from(item.run.advance_px / px(1.0)) / 2.0,
        baseline.y - f64::from(item.run.cap_height_px / px(1.0)) / 2.0,
    )
}

#[test]
fn letters_stand_at_the_points_the_renderer_published_scaled_to_the_picture_and_pressed_inside_it() {
    for orientation in [DeviceOrientation::Portrait, DeviceOrientation::LandscapeLeft] {
        let mut rig = vectorscope_page(orientation);
        assert_eq!(
            drawn_letters(&mut rig),
            0,
            "{orientation:?}: до подписей рендера букв нет"
        );
        let layout = *rig.screen.state().calibration().layout().expect("раскладка");
        let picture = layout.scope_image_rect_for(ScopeKind::Vectorscope);
        let request = desired_request(&rig);
        // Красная на 30 % слева, 20 % сверху; пурпурная в самом углу (рендер прижал бы её сам, но экран тоже держит
        // рамку текста внутри картинки).
        rig.act(published(
            request,
            vec![label(TargetColor::Red, 0.3, 0.2), label(TargetColor::Magenta, 1.0, 0.0)],
        ));
        rig.run(100);

        let red = drawn_centre(&mut rig, &format!("{orientation:?}"), "R");
        let expected_red = Point::new(
            picture.min_x() + 0.3 * picture.size.width,
            picture.min_y() + 0.2 * picture.size.height,
        );
        assert!(
            (red.x - expected_red.x).abs() < 0.5 && (red.y - expected_red.y).abs() < 0.5,
            "{orientation:?}: «R» в {red:?} вместо {expected_red:?}"
        );

        let magenta = drawn_centre(&mut rig, &format!("{orientation:?}"), "MG");
        assert!(
            magenta.x < picture.max_x() && magenta.y > picture.min_y(),
            "{orientation:?}: «MG» целиком внутри картинки, центр {magenta:?}, картинка {picture:?}"
        );
        assert!(
            magenta.x > picture.max_x() - 30.0 && magenta.y < picture.min_y() + 30.0,
            "{orientation:?}: «MG» прижата к тому же углу, а не унесена: {magenta:?}"
        );
        assert_eq!(
            drawn_letters(&mut rig),
            2,
            "{orientation:?}: нарисованы ровно две присланные буквы"
        );
    }
}

#[test]
fn letters_computed_for_another_request_are_not_drawn_after_a_zoom_axes_or_page_change() {
    let mut rig = vectorscope_page(DeviceOrientation::Portrait);
    let normal = desired_request(&rig);
    rig.act(published(normal, vec![label(TargetColor::Red, 0.3, 0.2)]));
    rig.run(100);
    assert_eq!(drawn_letters(&mut rig), 1, "буквы своего запроса рисуются");

    // Зум ×2: запрос другой, новых подписей рендер ещё не прислал — старые не стоят поверх нового прибора.
    rig.act(ScreenAction::VectorscopeZoomSelected(VectorscopeZoom::Double));
    rig.run(100);
    let zoomed = desired_request(&rig);
    assert_ne!(zoomed, normal);
    assert_eq!(drawn_letters(&mut rig), 0, "после зума ×2 чужие буквы убраны");

    // Подписи нового запроса — рисуются снова; они же исчезают при смене осей.
    rig.act(published(zoomed, vec![label(TargetColor::Blue, 0.8, 0.5)]));
    rig.run(100);
    assert_eq!(drawn_letters(&mut rig), 1);
    rig.act(ScreenAction::VectorscopeAxesSelected(VectorscopeAxes::OklabAb));
    rig.run(100);
    assert_eq!(drawn_letters(&mut rig), 0, "после смены осей чужие буквы убраны");

    // Свайп на другую страницу: на ней буквам вектороскопа не место, даже если подписи в хранилище остались.
    rig.act(ScreenAction::VectorscopeAxesSelected(VectorscopeAxes::YCbCrBt709));
    rig.run(100);
    assert_eq!(
        drawn_letters(&mut rig),
        1,
        "вернули запрос, под который подписи посчитаны, — буквы на месте"
    );
    rig.act(ScreenAction::ScopeKindSelected(ScopeKind::RgbParade));
    rig.run(300);
    assert_eq!(drawn_letters(&mut rig), 0, "на странице parade букв вектороскопа нет");
}

#[test]
fn a_closed_or_unpublished_board_draws_nothing() {
    let mut rig = vectorscope_page(DeviceOrientation::Portrait);
    let request = desired_request(&rig);
    rig.act(published(request, vec![label(TargetColor::Cyan, 0.5, 0.9)]));
    rig.run(100);
    assert_eq!(drawn_letters(&mut rig), 1);

    rig.act(ScreenAction::ScopeLabelsChanged(ScopeLabelsState::Withdrawn));
    rig.run(100);
    assert_eq!(drawn_letters(&mut rig), 0, "приборы закрыты — букв нет");

    rig.act(published(request, vec![label(TargetColor::Cyan, 0.5, 0.9)]));
    rig.act(ScreenAction::ScopeLabelsChanged(ScopeLabelsState::NotPublished));
    rig.run(100);
    assert_eq!(drawn_letters(&mut rig), 0, "«не публиковалось» букв не рисует");
}

/// Сколько раз нарисована подпись охвата «P3» (роль Caption).
fn drawn_p3(rig: &mut Rig) -> usize {
    let list = rig.frame().clone();
    text_count_of(&list, "P3", TextRoleKind::Caption)
}

#[test]
fn the_cie_page_draws_the_gamut_caption_inside_the_picture_and_a_diagram_switch_drops_stale_captions() {
    use scope_contract::{ChromaticityGamut, CieDiagram};
    let mut rig = calibrating(look_facts(), DeviceOrientation::Portrait);
    rig.act(ScreenAction::ScopeKindSelected(ScopeKind::Cie));
    rig.act(ScreenAction::CieDiagramSelected(CieDiagram::Xy1931));
    rig.run(300);
    assert_eq!(drawn_p3(&mut rig), 0, "до подписей рендера охват не подписан");

    let xy = desired_request(&rig);
    assert_eq!(xy.cie_diagram, CieDiagram::Xy1931);
    let gamut = ScopeLabel {
        mark: ScopeMark::Gamut(ChromaticityGamut::DisplayP3),
        at: LabelPoint::try_new(0.6, 0.4).expect("точка в картинке"),
    };
    rig.act(published(xy, vec![gamut]));
    rig.run(100);
    assert_eq!(drawn_p3(&mut rig), 1, "подпись «P3» нарисована один раз");
    let layout = *rig.screen.state().calibration().layout().expect("раскладка");
    let picture = layout.scope_image_rect_for(ScopeKind::Cie);
    let centre = drawn_centre(&mut rig, "Portrait", "P3");
    assert!(
        centre.x > picture.min_x()
            && centre.x < picture.max_x()
            && centre.y > picture.min_y()
            && centre.y < picture.max_y(),
        "«P3» внутри картинки CIE: {centre:?} против {picture:?}"
    );

    // Шкала u'v': запрос другой, рендер ещё не пересчитал — подписи xy на первом же кадре не рисуются.
    rig.act(ScreenAction::CieDiagramSelected(CieDiagram::UvPrime1976));
    rig.run(100);
    assert_ne!(desired_request(&rig), xy);
    assert_eq!(drawn_p3(&mut rig), 0, "после смены шкалы чужая подпись убрана сразу");

    // Подписи нового запроса — рисуются снова.
    rig.act(published(desired_request(&rig), vec![gamut]));
    rig.run(100);
    assert_eq!(drawn_p3(&mut rig), 1);
}
