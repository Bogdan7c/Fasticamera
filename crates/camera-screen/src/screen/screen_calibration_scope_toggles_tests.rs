//! Переключатели страниц гистограммы и CIE в полоске прибора (решения 331, 332, 335) — пальцем, как в руках: касание
//! в точках поверхности → состояние раздела и запрос рендеру. Каждый переключатель виден и ловит касания ТОЛЬКО на
//! своей странице; выбор переживает свайпы; остальное состояние раздела от него не меняется.

use scope_contract::{CieDiagram, HistogramMode, ScopeKind, VectorscopeAxes, VectorscopeZoom};
use shell_contract::orientation::DeviceOrientation;
use ui_kit::tokens::TextRoleKind;

use super::screen_calibration_tests::{calibrating, center};
use super::test_rig::*;
use crate::action::ScreenAction;
use crate::effect::Effect;
use crate::look_test_support::look_facts;

const ORIENTATIONS: [DeviceOrientation; 3] = [
    DeviceOrientation::Portrait,
    DeviceOrientation::LandscapeLeft,
    DeviceOrientation::LandscapeRight,
];

fn calibration(rig: &Rig) -> &crate::calibration::CalibrationState {
    rig.screen.state().calibration()
}

/// Запросы прибора, ушедшие рендеру с прошлого вызова.
fn scope_requests(rig: &mut Rig) -> Vec<scope_contract::ScopeRequest> {
    rig.effects()
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::Scopes(scope_contract::ScopeIntent::Show(request)) => Some(request),
            _ => None,
        })
        .collect()
}

fn select_page(rig: &mut Rig, kind: ScopeKind) {
    rig.act(ScreenAction::ScopeKindSelected(kind));
    rig.run(300);
}

#[test]
fn the_histogram_segments_pick_the_mode_and_the_request_carries_it_in_every_orientation() {
    for orientation in ORIENTATIONS {
        let mut rig = calibrating(look_facts(), orientation);
        assert_eq!(calibration(&rig).scope_kind(), ScopeKind::Histogram);
        assert_eq!(calibration(&rig).histogram_mode(), HistogramMode::Rgb);
        let list = rig.frame().clone();
        for label in ["RGB", "PARADE", "Y"] {
            assert_eq!(
                text_count_of(&list, label, TextRoleKind::SegmentLabel),
                1,
                "{orientation:?}: сегмент {label} нарисован"
            );
        }
        let before = calibration(&rig).clone();
        for (index, expected) in [
            (1, HistogramMode::Parade),
            (2, HistogramMode::Luma),
            (0, HistogramMode::Rgb),
        ] {
            rig.effects();
            rig.tap(center(&rig, "calib_scope_histogram_mode", index));
            rig.run(200);
            assert_eq!(
                calibration(&rig).histogram_mode(),
                expected,
                "{orientation:?}: сегмент {index}"
            );
            let requests = scope_requests(&mut rig);
            assert_eq!(requests.len(), 1, "{orientation:?}: рендеру ушёл один запрос");
            assert_eq!(requests[0].histogram_mode, expected);
            assert_eq!(requests[0].kind, ScopeKind::Histogram);
        }
        // Всё остальное в разделе осталось прежним.
        assert_eq!(calibration(&rig).vectorscope_axes(), before.vectorscope_axes());
        assert_eq!(calibration(&rig).vectorscope_zoom(), before.vectorscope_zoom());
        assert_eq!(calibration(&rig).cie_diagram(), before.cie_diagram());
        assert_eq!(calibration(&rig).scope_kind(), ScopeKind::Histogram);
        // Тап по уже выбранному сегменту рендеру ничего не шлёт.
        rig.effects();
        rig.tap(center(&rig, "calib_scope_histogram_mode", 0));
        rig.run(200);
        assert!(
            scope_requests(&mut rig).is_empty(),
            "{orientation:?}: повтор выбора молчит"
        );
    }
}

#[test]
fn the_histogram_mode_survives_a_swipe_away_and_back_and_the_toggle_is_dead_on_other_pages() {
    let mut rig = calibrating(look_facts(), DeviceOrientation::Portrait);
    let parade_spot = center(&rig, "calib_scope_histogram_mode", 1);
    let luma_spot = center(&rig, "calib_scope_histogram_mode", 2);
    rig.tap(luma_spot);
    rig.run(200);
    assert_eq!(calibration(&rig).histogram_mode(), HistogramMode::Luma);

    for other in [ScopeKind::Waveform, ScopeKind::Vectorscope, ScopeKind::Cie] {
        select_page(&mut rig, other);
        let list = rig.frame().clone();
        for label in ["PARADE", "RGB"] {
            assert_eq!(
                text_count_of(&list, label, TextRoleKind::SegmentLabel),
                0,
                "{other:?}: переключателя гистограммы нет"
            );
        }
        rig.effects();
        rig.tap(parade_spot);
        rig.run(200);
        assert_eq!(
            calibration(&rig).histogram_mode(),
            HistogramMode::Luma,
            "{other:?}: тап по месту сегмента режим не меняет"
        );
        assert_eq!(calibration(&rig).scope_kind(), other, "{other:?}: страница та же");
        assert_eq!(
            scope_requests(&mut rig)
                .iter()
                .filter(|request| request.histogram_mode != HistogramMode::Luma)
                .count(),
            0,
            "{other:?}: рендеру не ушло ничего с чужим режимом"
        );
    }
    rig.effects();
    select_page(&mut rig, ScopeKind::Histogram);
    let back = scope_requests(&mut rig);
    assert_eq!(
        back.last().map(|request| request.histogram_mode),
        Some(HistogramMode::Luma)
    );
    assert_eq!(
        calibration(&rig).histogram_mode(),
        HistogramMode::Luma,
        "выбор пережил свайпы"
    );
}

#[test]
fn the_cie_segments_pick_the_diagram_survive_swipes_and_are_dead_on_other_pages() {
    for orientation in ORIENTATIONS {
        let mut rig = calibrating(look_facts(), orientation);
        select_page(&mut rig, ScopeKind::Cie);
        assert_eq!(calibration(&rig).cie_diagram(), CieDiagram::Xy1931);
        let list = rig.frame().clone();
        assert_eq!(text_count_of(&list, "xy", TextRoleKind::SegmentLabel), 1);
        assert_eq!(text_count_of(&list, "u'v'", TextRoleKind::SegmentLabel), 1);
        let before = calibration(&rig).clone();
        let uv_spot = center(&rig, "calib_scope_cie_diagram", 1);
        rig.effects();
        rig.tap(uv_spot);
        rig.run(200);
        assert_eq!(
            calibration(&rig).cie_diagram(),
            CieDiagram::UvPrime1976,
            "{orientation:?}"
        );
        let requests = scope_requests(&mut rig);
        assert_eq!(requests.len(), 1, "{orientation:?}");
        assert_eq!(requests[0].cie_diagram, CieDiagram::UvPrime1976);
        assert_eq!(requests[0].kind, ScopeKind::Cie);
        assert_eq!(calibration(&rig).histogram_mode(), before.histogram_mode());
        assert_eq!(calibration(&rig).vectorscope_axes(), before.vectorscope_axes());
        assert_eq!(calibration(&rig).vectorscope_zoom(), before.vectorscope_zoom());

        // Уход со страницы: сегментов нет, касание по их месту ничего не меняет, выбор жив при возврате.
        select_page(&mut rig, ScopeKind::Vectorscope);
        let list = rig.frame().clone();
        assert_eq!(text_count_of(&list, "u'v'", TextRoleKind::SegmentLabel), 0);
        let xy_spot = center(&rig, "calib_scope_cie_diagram", 0);
        rig.tap(xy_spot);
        rig.run(200);
        assert_eq!(
            calibration(&rig).cie_diagram(),
            CieDiagram::UvPrime1976,
            "{orientation:?}: тап вхолостую"
        );
        assert_eq!(calibration(&rig).vectorscope_axes(), VectorscopeAxes::YCbCrBt709);
        assert_eq!(calibration(&rig).vectorscope_zoom(), VectorscopeZoom::Normal);
        select_page(&mut rig, ScopeKind::Cie);
        assert_eq!(calibration(&rig).cie_diagram(), CieDiagram::UvPrime1976);
        rig.tap(center(&rig, "calib_scope_cie_diagram", 0));
        rig.run(200);
        assert_eq!(
            calibration(&rig).cie_diagram(),
            CieDiagram::Xy1931,
            "{orientation:?}: назад на xy"
        );
    }
}

#[test]
fn the_config_sets_the_opening_page_the_histogram_mode_and_the_cie_diagram() {
    // По умолчанию — гистограмма, RGB, xy (решения 331, 332, 335).
    let defaults = crate::config::ScreenConfig::default();
    assert_eq!(defaults.calibration.opening_scope_page, ScopeKind::Histogram);
    let screen = crate::test_support::Screen::unprepared(defaults);
    assert_eq!(screen.state.calibration().scope_kind(), ScopeKind::Histogram);
    assert_eq!(screen.state.calibration().histogram_mode(), HistogramMode::Rgb);
    assert_eq!(screen.state.calibration().cie_diagram(), CieDiagram::Xy1931);

    let mut config = crate::config::ScreenConfig::default();
    config.calibration.opening_scope_page = ScopeKind::Waveform;
    config.calibration.histogram_mode = HistogramMode::Parade;
    config.calibration.cie_diagram = CieDiagram::UvPrime1976;
    let screen = crate::test_support::Screen::unprepared(config);
    assert_eq!(screen.state.calibration().scope_kind(), ScopeKind::Waveform);
    assert_eq!(screen.state.calibration().histogram_mode(), HistogramMode::Parade);
    assert_eq!(screen.state.calibration().cie_diagram(), CieDiagram::UvPrime1976);
    assert_eq!(
        screen
            .state
            .calibration()
            .desired_scope_request()
            .map(|r| r.histogram_mode),
        None,
        "раскладки до поверхности нет — просить нечего"
    );
}
