//! Подписи прибора сквозь все слои (решения 326, 327): действие на экране → запрос рендеру → НАСТОЯЩИЙ бэкенд
//! (RADV, headless) решает прибор и кладёт подписи на доску → приложение читает доску → экран рисует буквы целей
//! вектороскопа в точках рендера. Доказательство — буквы в рисунке экрана и запрос на доске, а не вызовы.
//!
//! Стенд — `calibration_rig` (общий с `calibration_end_to_end.rs`).

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

#[path = "../../render-loop/tests/support/mod.rs"]
mod support;
#[path = "../../render-loop/tests/viewfinder_rig/mod.rs"]
mod viewfinder_rig;
#[path = "../../render-loop/tests/vulkan_support/mod.rs"]
mod vulkan_support;

#[allow(
    dead_code,
    reason = "стенд общий: каждый тестовый файл берёт своё подмножество шагов"
)]
mod calibration_rig;

use calibration_rig::{AcceptingPersistence, CalibrationRig, library_over, live_scope_request};
use camera_screen::ScreenAction;
use camera_screen::modes::CaptureMode;
use scope_contract::{ScopeLabelsState, VectorscopeAxes, VectorscopeZoom};
use viewfinder_rig::reddish;

/// Буквы целей вектороскопа в порядке, в котором их называет решение 326.
const LETTERS: [&str; 6] = ["R", "MG", "B", "CY", "G", "YL"];

fn calibrating_rig() -> Option<CalibrationRig> {
    let mut rig = CalibrationRig::start(library_over(Box::new(AcceptingPersistence)), reddish())?;
    rig.send(ScreenAction::ModeSelected(CaptureMode::Calibrate));
    rig.frames(3);
    Some(rig)
}

/// Сколько букв целей нарисовано на последнем кадре экрана.
fn drawn_letters(rig: &CalibrationRig) -> usize {
    LETTERS.iter().map(|text| rig.drawn_caption_origins(text).len()).sum()
}

fn published_request_on_the_board(rig: &CalibrationRig) -> Option<scope_contract::ScopeRequest> {
    match rig.bench.scope_labels.latest() {
        ScopeLabelsState::Published(labels) => Some(labels.request),
        ScopeLabelsState::NotPublished | ScopeLabelsState::Withdrawn => None,
    }
}

#[test]
fn the_vectorscope_page_shows_all_six_letters_where_the_render_computed_them_and_axes_move_them() {
    let Some(mut rig) = calibrating_rig() else {
        return;
    };
    rig.send(ScreenAction::ScopeKindSelected(scope_contract::ScopeKind::Vectorscope));
    rig.send(ScreenAction::VectorscopeAxesSelected(VectorscopeAxes::YCbCrBt709));
    rig.frames(5);
    let ycbcr = live_scope_request(&mut rig);
    assert_eq!(ycbcr.kind, scope_contract::ScopeKind::Vectorscope);
    assert_eq!(
        published_request_on_the_board(&rig),
        Some(ycbcr),
        "рендер посчитал подписи для запроса, который держит"
    );
    for letter in LETTERS {
        assert_eq!(
            rig.drawn_caption_origins(letter).len(),
            1,
            "буква «{letter}» нарисована один раз"
        );
    }
    let red_on_ycbcr = rig.drawn_caption_origins("R")[0];

    // Другие оси: цели стоят в других местах, буквы следуют за ними.
    rig.send(ScreenAction::VectorscopeAxesSelected(VectorscopeAxes::OklabAb));
    rig.frames(5);
    assert_eq!(live_scope_request(&mut rig).vectorscope_axes, VectorscopeAxes::OklabAb);
    assert_eq!(drawn_letters(&rig), 6);
    let red_on_oklab = rig.drawn_caption_origins("R")[0];
    let moved = (red_on_ycbcr.0 - red_on_oklab.0).hypot(red_on_ycbcr.1 - red_on_oklab.1);
    assert!(
        moved > 3.0,
        "красная цель на OKLab стоит в другом месте, чем на Y'CbCr: сдвиг {moved} px"
    );
    rig.finish();
}

#[test]
fn zoom_removes_the_old_letters_at_once_and_leaving_calibrate_withdraws_the_labels() {
    let Some(mut rig) = calibrating_rig() else {
        return;
    };
    rig.send(ScreenAction::ScopeKindSelected(scope_contract::ScopeKind::Vectorscope));
    rig.send(ScreenAction::VectorscopeZoomSelected(VectorscopeZoom::Normal));
    rig.frames(5);
    assert_eq!(drawn_letters(&rig), 6);

    // Зум ×2 при умолчаниях выносит все цели за сетку: на ПЕРВОМ же кадре после нажатия рендер ещё не успел ничего
    // опубликовать, но чужие (прежнего зума) буквы экран уже не рисует.
    rig.send(ScreenAction::VectorscopeZoomSelected(VectorscopeZoom::Double));
    rig.frame();
    assert_eq!(drawn_letters(&rig), 0, "после зума ×2 прежние буквы убраны сразу");
    rig.frames(4);
    assert_eq!(live_scope_request(&mut rig).vectorscope_zoom, VectorscopeZoom::Double);
    assert_eq!(
        published_request_on_the_board(&rig),
        Some(live_scope_request(&mut rig)),
        "подписи на доске следуют за запросом (их число — ноль: все цели за сеткой)"
    );
    assert_eq!(drawn_letters(&rig), 0);

    // Выход из CALIBRATE закрывает приборы: подписи сняты с доски.
    rig.send(ScreenAction::ModeSelected(CaptureMode::Photo));
    rig.frames(4);
    assert_eq!(rig.bench.scope_labels.latest(), ScopeLabelsState::Withdrawn);
    rig.finish();
}
