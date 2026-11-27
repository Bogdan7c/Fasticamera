//! Подписи прибора (решения 326, 327) через НАСТОЯЩИЙ бэкенд показа: рендер кладёт на доску подписи целей
//! вектороскопа с запросом, для которого они посчитаны; свайп на другую страницу и зум ×2 меняют запрос на доске,
//! закрытие приборов снимает подписи. Стенд тот же, что у `vulkan_scopes_demand.rs` (RADV, headless, sRGB-выход).

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod scope_rig;
mod support;
mod viewfinder_rig;
mod vulkan_support;

use render_loop::viewfinder::ViewfinderCommandOutcome;
use scope_contract::{
    ScopeIntent, ScopeKind, ScopeLabels, ScopeLabelsBoard, ScopeLabelsState, ScopeMark, ScopeRequest, TargetColor,
    VectorscopeAxes, VectorscopeZoom,
};
use scope_rig::{WARM_UP_FRAMES, apply_scopes, request, start, waveform};
use viewfinder_rig::reddish;

fn published(board: &ScopeLabelsBoard) -> ScopeLabels {
    match board.latest() {
        ScopeLabelsState::Published(labels) => labels,
        other => panic!("ждали подписи на доске, получили {other:?}"),
    }
}

fn vectorscope(axes: VectorscopeAxes, zoom: VectorscopeZoom) -> ScopeRequest {
    ScopeRequest {
        vectorscope_zoom: zoom,
        cie_diagram: scope_contract::CieDiagram::default(),
        histogram_mode: scope_contract::HistogramMode::default(),
        ..request(ScopeKind::Vectorscope, axes, 160, 160)
    }
}

#[test]
fn the_board_follows_the_real_scopes_request_page_zoom_and_hide() {
    let scenes = vec![reddish(); WARM_UP_FRAMES + 10];
    let Some(mut bench) = start(&scenes) else { return };
    let board = bench.scope_labels.clone();
    assert_eq!(
        board.latest(),
        ScopeLabelsState::NotPublished,
        "до приборов подписей нет"
    );

    // Вектороскоп Y'CbCr: шесть букв, каждая со своим цветом, запрос на доске — тот, что показан.
    let ycbcr = vectorscope(VectorscopeAxes::YCbCrBt709, VectorscopeZoom::Normal);
    assert_eq!(
        apply_scopes(&mut bench, ScopeIntent::Show(ycbcr)),
        ViewfinderCommandOutcome::Applied
    );
    bench.show_next_frame();
    let first = published(&board);
    assert_eq!(first.request, ycbcr);
    let marks: Vec<ScopeMark> = first.labels.iter().map(|label| label.mark).collect();
    for color in [
        TargetColor::Red,
        TargetColor::Magenta,
        TargetColor::Blue,
        TargetColor::Cyan,
        TargetColor::Green,
        TargetColor::Yellow,
    ] {
        assert!(
            marks.contains(&ScopeMark::VectorscopeTarget(color)),
            "{color:?} в {marks:?}"
        );
    }

    // Другие оси — другой запрос и другие места букв.
    let oklab = vectorscope(VectorscopeAxes::OklabAb, VectorscopeZoom::Normal);
    assert_eq!(
        apply_scopes(&mut bench, ScopeIntent::Show(oklab)),
        ViewfinderCommandOutcome::Applied
    );
    bench.show_next_frame();
    let second = published(&board);
    assert_eq!(second.request, oklab);
    assert_ne!(
        second.labels, first.labels,
        "на других осях буквы стоят в других местах"
    );

    // Зум ×2 при умолчаниях выносит все цели за сетку: запрос новый, букв нет.
    let zoomed = vectorscope(VectorscopeAxes::YCbCrBt709, VectorscopeZoom::Double);
    assert_eq!(
        apply_scopes(&mut bench, ScopeIntent::Show(zoomed)),
        ViewfinderCommandOutcome::Applied
    );
    bench.show_next_frame();
    let third = published(&board);
    assert_eq!(third.request, zoomed);
    assert!(third.labels.is_empty(), "{third:?}");

    // Свайп на waveform: чужих букв не остаётся, запрос помечен новый.
    assert_eq!(
        apply_scopes(&mut bench, ScopeIntent::Show(waveform())),
        ViewfinderCommandOutcome::Applied
    );
    bench.show_next_frame();
    let fourth = published(&board);
    assert_eq!(fourth.request, waveform());
    assert!(fourth.labels.is_empty());

    // Закрытие приборов снимает подписи сразу.
    assert_eq!(
        apply_scopes(&mut bench, ScopeIntent::Hide),
        ViewfinderCommandOutcome::Applied
    );
    assert_eq!(board.latest(), ScopeLabelsState::Withdrawn);
    bench.finish();
}
