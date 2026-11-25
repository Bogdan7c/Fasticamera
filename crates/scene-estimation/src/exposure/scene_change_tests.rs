//! Детектор смены сцены на синтетических кадрах: что он подтверждает, а что обязан пропустить молча.
//!
//! Сцена задаётся светом на плитку «на единицу произведения» (`scene`), кадр с произведением `P` видит
//! `уровень = scene · P` (выше порога пересвета плитка пересвечена целиком — как в настоящей статистике).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

use camera_contract::geometry::{PixelRect, PixelSize};
use isp_model::statistics::{AwbStatistics, AwbTile, TileGrid};

use super::*;
use crate::exposure::test_support::{GRID, product, stats_from, stats_from_tiles, tile_of};

/// Кадр сетки 32×24 с сценой `scene(column, row)` (свет на единицу произведения) при произведении `product`.
fn frame_of(scene: impl Fn(u32, u32) -> f32, product: f64) -> AwbStatistics {
    stats_from(|column, row| {
        let level = scene(column, row) * product as f32;
        [level; 3]
    })
}

/// Сцена, одинаковая на всех плитках.
fn flat(light: f32) -> impl Fn(u32, u32) -> f32 {
    move |_, _| light
}

fn framing() -> PixelRect {
    PixelRect {
        left: 0,
        top: 0,
        size: PixelSize::new(4000, 3000),
    }
}

/// Стенд: детектор, один тап и часы (метки сенсора).
struct Rig {
    detector: SceneChangeDetector,
    request: PointRequestId,
    now_ms: i64,
    framing: PixelRect,
    step_ms: i64,
}

impl Rig {
    fn new() -> Self {
        Self::with_config(SceneChangeConfig::default())
    }

    fn with_config(config: SceneChangeConfig) -> Self {
        Self {
            detector: SceneChangeDetector::new(config).unwrap(),
            request: PointRequestId(1),
            now_ms: 1_000,
            framing: framing(),
            step_ms: 100,
        }
    }

    /// Кадр с меткой «сейчас»; часы идут на `step_ms`.
    fn feed(&mut self, statistics: &AwbStatistics, frame_product: f64) -> SceneWatchStep {
        let step = self.detector.observe(&SceneFrame {
            request: self.request,
            statistics,
            product: product(frame_product),
            sensor_timestamp: SensorTimestamp::from_nanos(self.now_ms * 1_000_000),
            framing: self.framing,
        });
        self.now_ms += self.step_ms;
        step
    }

    /// Снять эталон сразу (AE устоялся).
    fn take_reference(&mut self, statistics: &AwbStatistics, frame_product: f64) {
        assert_eq!(
            self.feed(statistics, frame_product),
            SceneWatchStep::ReferenceTaken(ReferenceReason::FirstFrame)
        );
    }

    /// Кадр без обновления «устоялся».
    fn watch(&mut self, statistics: &AwbStatistics, frame_product: f64) -> SceneWatchStep {
        self.feed(statistics, frame_product)
    }
}

fn is_confirmed(step: SceneWatchStep) -> bool {
    matches!(step, SceneWatchStep::Confirmed(_))
}

/// Среди шагов нет ничего, кроме «похож» или «вывода нет»: детектор не заподозрил смену.
fn assert_never_suspicious(steps: &[SceneWatchStep]) {
    for step in steps {
        assert!(
            matches!(step, SceneWatchStep::Similar | SceneWatchStep::Inconclusive),
            "детектор заподозрил смену на неподвижной сцене: {step:?}"
        );
    }
}

#[test]
fn a_still_scene_stays_similar_for_as_long_as_you_like() {
    let mut rig = Rig::new();
    let scene = frame_of(flat(0.2), 1.0);
    rig.take_reference(&scene, 1.0);
    let steps: Vec<_> = (0..2_000).map(|_| rig.watch(&scene, 1.0)).collect();
    assert!(
        steps.iter().all(|step| *step == SceneWatchStep::Similar),
        "{:?}",
        steps.last()
    );
}

#[test]
fn exposure_changes_of_the_same_scene_do_not_look_like_a_change_even_with_clipped_parts() {
    // Четверть кадра — светлое окно: при P и 2P оно пересвечено, при ½P измеряется. Остальное — обычная комната.
    let scene = |column: u32, row: u32| if column >= 24 && row < 12 { 1.5 } else { 0.2 };
    for reference_product in [0.5, 1.0, 2.0] {
        let mut rig = Rig::new();
        rig.take_reference(&frame_of(scene, reference_product), reference_product);
        let steps: Vec<_> = [0.5, 1.0, 2.0, 1.0, 0.5, 2.0]
            .into_iter()
            .map(|current| rig.watch(&frame_of(scene, current), current))
            .collect();
        assert_never_suspicious(&steps);
        assert!(
            steps.contains(&SceneWatchStep::Similar),
            "голосов хватает: комната измерена и в эталоне, и в кадре: {steps:?}"
        );
    }
}

#[test]
fn mains_flicker_of_ten_percent_is_not_a_change() {
    let mut rig = Rig::new();
    rig.take_reference(&frame_of(flat(0.3), 1.0), 1.0);
    let steps: Vec<_> = (0..200)
        .map(|index| {
            let flicker = if index % 2 == 0 { 0.9 } else { 1.1 };
            rig.watch(&frame_of(flat(0.3 * flicker), 1.0), 1.0)
        })
        .collect();
    assert!(steps.iter().all(|step| *step == SceneWatchStep::Similar), "{steps:?}");
}

#[test]
fn a_hand_shake_shifting_the_picture_by_a_tile_is_not_a_change() {
    // Резкая граница светлого и тёмного; дрожь рук сдвигает её на плитку (четверть ячейки грубой сетки).
    let edge_at = |edge: u32| move |column: u32, _row: u32| if column < edge { 0.05 } else { 0.4 };
    let mut rig = Rig::new();
    rig.take_reference(&frame_of(edge_at(16), 1.0), 1.0);
    for shifted_edge in [15, 17, 16, 17, 15, 16].into_iter().cycle().take(60) {
        let step = rig.watch(&frame_of(edge_at(shifted_edge), 1.0), 1.0);
        assert_eq!(step, SceneWatchStep::Similar, "сдвиг границы на плитку не смена сцены");
    }
}

#[test]
fn a_passer_by_covering_a_quarter_to_a_third_of_the_frame_does_not_drop_the_point() {
    // Прохожий — тёмный прямоугольник на 3 EV темнее. Грубая ячейка = 4×4 плитки.
    for (columns, rows) in [(12, 12), (16, 14)] {
        let mut rig = Rig::new();
        rig.take_reference(&frame_of(flat(0.5), 1.0), 1.0);
        let with_passer_by = |column: u32, row: u32| if column < columns && row < rows { 0.0625 } else { 0.5 };
        let steps: Vec<_> = (0..100)
            .map(|_| rig.watch(&frame_of(with_passer_by, 1.0), 1.0))
            .collect();
        assert!(
            steps.iter().all(|step| *step == SceneWatchStep::Similar),
            "прохожий на {columns}×{rows} плиток ({:.0} % кадра) снял точку: {:?}",
            100.0 * f64::from(columns * rows) / 768.0,
            steps.last()
        );
    }
}

#[test]
fn a_change_shorter_than_the_confirmation_time_is_not_confirmed_and_a_similar_frame_restarts_it() {
    let mut rig = Rig::new();
    let calm = frame_of(flat(0.4), 1.0);
    let dark = frame_of(flat(0.05), 1.0);
    rig.take_reference(&calm, 1.0);
    // Четыре кадра подряд «не похож» (300 мс от первого): мало и по времени (500 мс), и по числу.
    let brief: Vec<_> = (0..4).map(|_| rig.watch(&dark, 1.0)).collect();
    assert!(brief.iter().all(|step| *step == SceneWatchStep::Diverging), "{brief:?}");
    assert_eq!(rig.watch(&calm, 1.0), SceneWatchStep::Similar);
    // Один «похожий» кадр сбросил отсчёт: ещё четыре «не похож» снова не подтверждают.
    let again: Vec<_> = (0..4).map(|_| rig.watch(&dark, 1.0)).collect();
    assert!(again.iter().all(|step| *step == SceneWatchStep::Diverging), "{again:?}");
}

#[test]
fn a_real_change_is_confirmed_exactly_after_the_confirmation_time_and_frames() {
    let mut rig = Rig::new();
    rig.take_reference(&frame_of(flat(0.4), 1.0), 1.0);
    let other = frame_of(flat(0.05), 1.0);
    // Кадры через 100 мс: подтверждение — на кадре, где «не похож» держится 500 мс.
    let steps: Vec<_> = (0..6).map(|_| rig.watch(&other, 1.0)).collect();
    assert!(
        steps[..5].iter().all(|step| *step == SceneWatchStep::Diverging),
        "{steps:?}"
    );
    assert_eq!(
        steps[5],
        SceneWatchStep::Confirmed(ChangeEvidence {
            changed_cells: 48,
            voting_cells: 48
        })
    );
    // Пока условие держится, подтверждение повторяется на каждом кадре.
    assert!(is_confirmed(rig.watch(&other, 1.0)));
}

#[test]
fn sparse_frames_do_not_confirm_by_time_alone() {
    let mut rig = Rig::new();
    rig.step_ms = 300;
    rig.take_reference(&frame_of(flat(0.4), 1.0), 1.0);
    let other = frame_of(flat(0.05), 1.0);
    // Два кадра с разрывом 300 мс: 300 мс < 500; третий — 600 мс, но только третий кадр, и он подтверждает.
    assert_eq!(rig.watch(&other, 1.0), SceneWatchStep::Diverging);
    assert_eq!(rig.watch(&other, 1.0), SceneWatchStep::Diverging);
    // Время (600 мс) вышло, число кадров (3) набрано только сейчас.
    assert!(is_confirmed(rig.watch(&other, 1.0)));

    let mut rig = Rig::new();
    rig.step_ms = 600;
    rig.take_reference(&frame_of(flat(0.4), 1.0), 1.0);
    // Время давно вышло, но кадров всего два: два редких кадра смены не доказывают.
    assert_eq!(rig.watch(&other, 1.0), SceneWatchStep::Diverging);
    assert_eq!(rig.watch(&other, 1.0), SceneWatchStep::Diverging);
    assert!(is_confirmed(rig.watch(&other, 1.0)), "третий кадр подтверждает");
}

#[test]
fn panning_to_another_scene_is_confirmed_even_when_the_point_itself_stays_equally_dark() {
    // Точка тапа — центр кадра (4 ячейки в середине), там темно и до, и после; вокруг сцена сменилась.
    let centre = |column: u32, row: u32| (12..20).contains(&column) && (8..16).contains(&row);
    let before = |column: u32, row: u32| if centre(column, row) { 0.05 } else { 0.3 };
    let after = |column: u32, row: u32| if centre(column, row) { 0.05 } else { 0.9 };
    let mut rig = Rig::new();
    rig.take_reference(&frame_of(before, 0.5), 0.5);
    let steps: Vec<_> = (0..8).map(|_| rig.watch(&frame_of(after, 0.5), 0.5)).collect();
    assert!(is_confirmed(*steps.last().unwrap()), "{steps:?}");
    let SceneWatchStep::Confirmed(evidence) = *steps.last().unwrap() else {
        unreachable!()
    };
    // Центр (2×2 ячейки) не изменился и проголосовал «так же»: изменились остальные 44 из 48.
    assert_eq!((evidence.changed_cells, evidence.voting_cells), (44, 48));
}

#[test]
fn turning_the_light_on_in_the_whole_scene_is_a_change() {
    let mut rig = Rig::new();
    rig.take_reference(&frame_of(flat(0.05), 1.0), 1.0);
    // Всё ×4 (+2 EV) при прежней выдержке: AE ещё не успел среагировать.
    let steps: Vec<_> = (0..8).map(|_| rig.watch(&frame_of(flat(0.2), 1.0), 1.0)).collect();
    assert!(is_confirmed(*steps.last().unwrap()), "{steps:?}");
}

#[test]
fn a_scene_that_was_solid_black_in_the_reference_neither_triggers_nor_sticks_forever() {
    let mut rig = Rig::new();
    let black = frame_of(flat(0.0), 1.0);
    rig.take_reference(&black, 1.0);
    // Чёрное против чёрного: голосовать нечем — вывода нет, и точку не снимаем.
    let steps: Vec<_> = (0..100).map(|_| rig.watch(&black, 1.0)).collect();
    assert!(
        steps.iter().all(|step| *step == SceneWatchStep::Inconclusive),
        "{:?}",
        steps.last()
    );
    // Включили свет: любая измеренная ячейка выше шума против «темно» — однозначная смена.
    let lit: Vec<_> = (0..8).map(|_| rig.watch(&frame_of(flat(0.2), 1.0), 1.0)).collect();
    assert!(is_confirmed(*lit.last().unwrap()), "{lit:?}");
}

#[test]
fn a_frame_without_enough_voting_cells_gives_no_conclusion_and_never_a_change() {
    // Почти весь кадр пересвечен и в эталоне, и сейчас (голосовать нечем); измерены лишь 8 ячеек, и они изменились.
    let measured_corner = |column: u32, row: u32| column < 16 && row < 8;
    let reference = |column: u32, row: u32| if measured_corner(column, row) { 0.4 } else { 5.0 };
    let current = |column: u32, row: u32| if measured_corner(column, row) { 0.05 } else { 5.0 };
    let mut rig = Rig::new();
    rig.take_reference(&frame_of(reference, 1.0), 1.0);
    let steps: Vec<_> = (0..40).map(|_| rig.watch(&frame_of(current, 1.0), 1.0)).collect();
    assert!(
        steps
            .iter()
            .all(|step| matches!(step, SceneWatchStep::Inconclusive | SceneWatchStep::ReferenceTaken(_))),
        "{steps:?}"
    );
    assert!(!steps.iter().any(|step| is_confirmed(*step)));
}

#[test]
fn zooming_retakes_the_reference_instead_of_reporting_a_change() {
    let mut rig = Rig::new();
    rig.take_reference(&frame_of(flat(0.4), 1.0), 1.0);
    // Зум: плитки легли на другой участок сцены, свет совсем другой.
    rig.framing = PixelRect {
        left: 1000,
        top: 750,
        size: PixelSize::new(2000, 1500),
    };
    let zoomed = frame_of(flat(0.03), 1.0);
    assert_eq!(
        rig.watch(&zoomed, 1.0),
        SceneWatchStep::ReferenceTaken(ReferenceReason::Reframed)
    );
    let steps: Vec<_> = (0..40).map(|_| rig.watch(&zoomed, 1.0)).collect();
    assert!(steps.iter().all(|step| *step == SceneWatchStep::Similar), "{steps:?}");
    // Настоящая смена после зума снова ловится по новому эталону.
    let steps: Vec<_> = (0..8).map(|_| rig.watch(&frame_of(flat(0.3), 1.0), 1.0)).collect();
    assert!(is_confirmed(*steps.last().unwrap()), "{steps:?}");
}

#[test]
fn a_new_tap_and_a_reset_start_over_and_old_state_does_not_leak() {
    let mut rig = Rig::new();
    rig.take_reference(&frame_of(flat(0.4), 1.0), 1.0);
    let other = frame_of(flat(0.05), 1.0);
    let steps: Vec<_> = (0..8).map(|_| rig.watch(&other, 1.0)).collect();
    assert!(is_confirmed(*steps.last().unwrap()));
    // Новый тап: старое подтверждение и эталон не действуют, эталон — заново.
    rig.request = PointRequestId(2);
    // Первый же кадр нового тапа — эталон (с нынешней сцены).
    rig.take_reference(&other, 1.0);
    assert_eq!(rig.watch(&other, 1.0), SceneWatchStep::Similar);
    // Новый поток: забываем всё.
    rig.detector.reset();
    assert_eq!(
        rig.feed(&other, 1.0),
        SceneWatchStep::ReferenceTaken(ReferenceReason::FirstFrame),
        "после сброса первый кадр — снова эталон"
    );
}

#[test]
fn a_frame_out_of_order_is_ignored_and_does_not_count_towards_confirmation() {
    let mut rig = Rig::new();
    rig.take_reference(&frame_of(flat(0.4), 1.0), 1.0);
    let other = frame_of(flat(0.05), 1.0);
    assert_eq!(rig.watch(&other, 1.0), SceneWatchStep::Diverging);
    // Тот же кадр ещё раз (метка не новее виденной).
    rig.now_ms -= rig.step_ms;
    assert_eq!(rig.watch(&other, 1.0), SceneWatchStep::Ignored);
    // Кадры 2 и 3: подтверждения нет — повторный кадр не набрал число подряд идущих.
    assert_eq!(rig.watch(&other, 1.0), SceneWatchStep::Diverging);
    assert_eq!(rig.watch(&other, 1.0), SceneWatchStep::Diverging);
}

#[test]
fn white_balance_shifts_do_not_move_the_signature() {
    // Меняется только красный и синий (смена белой / WARMTH / TINT); зелёный и свет те же.
    let base = |red: u32, blue: u32| {
        stats_from_tiles(move |_, _| {
            let tile = tile_of([0.3; 3]);
            AwbTile {
                sum_r: tile.sum_r / 100 * red,
                sum_b: tile.sum_b / 100 * blue,
                ..tile
            }
        })
    };
    let mut rig = Rig::new();
    rig.take_reference(&base(100, 100), 1.0);
    for (red, blue) in [(150, 60), (60, 160), (200, 30)] {
        for _ in 0..10 {
            assert_eq!(rig.watch(&base(red, blue), 1.0), SceneWatchStep::Similar);
        }
    }
}

#[test]
fn a_statistics_grid_that_does_not_divide_into_the_coarse_grid_still_detects_a_change() {
    // 5×5 плиток (меньше сетки 8×6 и не делится на неё): подпись 5×5 ячеек, смена всё равно ловится.
    let grid = TileGrid { columns: 5, rows: 5 };
    let uniform = |level: f32| {
        let tile = tile_of([level; 3]);
        AwbStatistics::from_tiles(grid, vec![AwbTile { ..tile }; 25]).unwrap()
    };
    let mut rig = Rig::new();
    rig.take_reference(&uniform(0.4), 1.0);
    assert_eq!(rig.watch(&uniform(0.4), 1.0), SceneWatchStep::Similar);
    let steps: Vec<_> = (0..8).map(|_| rig.watch(&uniform(0.03), 1.0)).collect();
    let SceneWatchStep::Confirmed(evidence) = *steps.last().unwrap() else {
        panic!("{steps:?}")
    };
    assert_eq!((evidence.changed_cells, evidence.voting_cells), (25, 25));
}

#[test]
fn the_test_grid_divides_into_the_coarse_grid_evenly() {
    // Сетка 32×24 делится на 8×6 нацело (ячейка 4×4 плитки): тесты выше рассчитывают на это.
    assert_eq!((GRID.columns % 8, GRID.rows % 6), (0, 0));
}

// ---- доработка S1b: эталон с первого кадра, уточнение, зум-подтверждение, малая сетка ----

/// Сцена с левой половиной и правой половиной разной яркости (на единицу произведения).
fn halves(left: f32, right: f32) -> impl Fn(u32, u32) -> f32 {
    move |column, _| if column < 16 { left } else { right }
}

#[test]
fn the_reference_is_the_first_frame_of_the_tap_and_does_not_wait_for_anything() {
    let mut rig = Rig::new();
    assert_eq!(
        rig.feed(&frame_of(flat(0.3), 1.0), 1.0),
        SceneWatchStep::ReferenceTaken(ReferenceReason::FirstFrame)
    );
    // Кадр, на котором нет ни одной ячейки со светом, годным не считается: ждём.
    let mut blank = Rig::new();
    let empty = AwbStatistics::from_tiles(TileGrid { columns: 4, rows: 4 }, vec![AwbTile::default(); 16]).unwrap();
    assert_eq!(blank.feed(&empty, 1.0), SceneWatchStep::AwaitingReference);
    assert_eq!(
        blank.feed(&frame_of(flat(0.3), 1.0), 1.0),
        SceneWatchStep::ReferenceTaken(ReferenceReason::FirstFrame)
    );
}

#[test]
fn a_tap_followed_at_once_by_another_scene_is_confirmed_while_the_exposure_is_still_converging() {
    // Тапнул и сразу увёл камеру: эталон — первый кадр (тёмный старт, выдержка 0,25), через два кадра сцена другая, а
    // выдержка ещё сходится (растёт к 1,0 и дальше).
    let mut rig = Rig::new();
    rig.step_ms = 67;
    let scene_a = halves(0.3, 0.3);
    let scene_b = halves(0.03, 0.03);
    rig.take_reference(&frame_of(&scene_a, 0.25), 0.25);
    assert_eq!(rig.watch(&frame_of(&scene_a, 0.25), 0.25), SceneWatchStep::Similar);
    let mut steps = Vec::new();
    for index in 0..14 {
        let exposure = 0.25 * (1.0 + index as f64 * 0.25);
        steps.push(rig.watch(&frame_of(&scene_b, exposure), exposure));
    }
    assert!(steps.iter().any(|step| is_confirmed(*step)), "{steps:?}");
}

#[test]
fn a_dark_reference_is_revealed_by_the_exposure_and_neither_publishes_nor_blocks_a_later_real_change() {
    // Тап при тёмном старте: эталон почти весь «не больше» (уровень ниже шума), затем выдержка растёт на +5 EV.
    let mut rig = Rig::new();
    let scene = flat(0.3);
    rig.take_reference(&frame_of(&scene, 0.01), 0.01);
    let mut steps = Vec::new();
    for exposure in [0.01, 0.02, 0.04, 0.08, 0.16, 0.32, 0.32, 0.32, 0.32, 0.32] {
        steps.push(rig.watch(&frame_of(&scene, exposure), exposure));
    }
    assert_never_suspicious(&steps);
    // Эталон раскрылся измеренным светом: теперь настоящая смена сцены видна.
    let steps: Vec<_> = (0..8).map(|_| rig.watch(&frame_of(flat(0.03), 0.32), 0.32)).collect();
    assert!(is_confirmed(*steps.last().unwrap()), "{steps:?}");
}

#[test]
fn a_clipped_reference_is_revealed_when_the_exposure_comes_down_and_a_later_change_is_still_found() {
    // Тап по светлому при светлом старте: эталон весь пересвечен, затем выдержка падает в 8 раз.
    let mut rig = Rig::new();
    let scene = flat(0.3);
    rig.take_reference(&frame_of(&scene, 8.0), 8.0);
    let mut steps = Vec::new();
    for exposure in [8.0, 4.0, 2.0, 1.0, 1.0, 1.0, 1.0, 1.0] {
        steps.push(rig.watch(&frame_of(&scene, exposure), exposure));
    }
    assert_never_suspicious(&steps);
    // Мягче прежнего порога «светлее эталона на 1,3 EV»: по нераскрытой границе (≥ 3 EV ниже) такая смена не видна.
    let steps: Vec<_> = (0..8).map(|_| rig.watch(&frame_of(flat(0.12), 1.0), 1.0)).collect();
    assert!(is_confirmed(*steps.last().unwrap()), "{steps:?}");
}

#[test]
fn a_slow_drift_of_the_scene_is_eventually_confirmed_because_measured_cells_are_never_rewritten() {
    // Сцена темнеет на 0,05 EV за кадр (кадры через 100 мс): медленный уход не должен съесть эталон.
    let mut rig = Rig::new();
    rig.take_reference(&frame_of(flat(0.4), 1.0), 1.0);
    let mut steps = Vec::new();
    for index in 1..=60 {
        let light = 0.4 * (-0.05 * f64::from(index)).exp2() as f32;
        steps.push(rig.watch(&frame_of(flat(light), 1.0), 1.0));
    }
    let first_confirmed = steps
        .iter()
        .position(|step| is_confirmed(*step))
        .expect("в итоге публикуется");
    // Порог 0,5 EV достигнут около 10-го кадра; подтверждение — через 500 мс после первого «не похож».
    assert!(
        (10..=24).contains(&first_confirmed),
        "подтверждено на кадре {first_confirmed}"
    );
}

#[test]
fn the_reference_is_not_refined_while_frames_vote_for_a_change() {
    // Эталон: слева измерено 0,4, справа окно (пересвет, свет «не меньше»). Сцена сменилась: слева темно, справа стена
    // 0,8 (измерена, но против «не меньше» вывода нет). Пока кадры голосуют «сменилась», окно эталона раскрываться
    // не имеет права.
    let mut rig = Rig::new();
    rig.take_reference(&frame_of(halves(0.4, 5.0), 1.0), 1.0);
    for _ in 0..3 {
        assert_eq!(
            rig.watch(&frame_of(halves(0.03, 0.8), 1.0), 1.0),
            SceneWatchStep::Diverging
        );
    }
    // Вернулась прежняя левая часть, а справа стена 0,7: относительно НЕРАСКРЫТОГО окна (≥ 1,0) это на ≥ 0,5 EV темнее —
    // половина кадра сменилась; раскрытое во время смены окно (0,8) приняло бы 0,7 за «то же».
    let step = rig.watch(&frame_of(halves(0.4, 0.7), 1.0), 1.0);
    assert!(
        matches!(step, SceneWatchStep::Diverging | SceneWatchStep::Confirmed(_)),
        "{step:?}"
    );
}

#[test]
fn refining_never_rewrites_a_measured_cell_even_when_the_frame_is_similar() {
    // Эталон весь измерен (0,4). Кадры «похожи» на 0,3 EV ниже (внутри порога) — это НЕ новый эталон: накопленный уход
    // всё равно должен дойти до порога относительно ИСХОДНОГО.
    let mut rig = Rig::new();
    rig.take_reference(&frame_of(flat(0.4), 1.0), 1.0);
    let slightly_darker = 0.4 * (-0.3_f32).exp2();
    for _ in 0..20 {
        assert_eq!(
            rig.watch(&frame_of(flat(slightly_darker), 1.0), 1.0),
            SceneWatchStep::Similar
        );
    }
    let darker = 0.4 * (-0.7_f32).exp2();
    let steps: Vec<_> = (0..8).map(|_| rig.watch(&frame_of(flat(darker), 1.0), 1.0)).collect();
    assert!(is_confirmed(*steps.last().unwrap()), "{steps:?}");
}

#[test]
fn zooming_during_an_unconfirmed_change_confirms_it_instead_of_forgetting_it() {
    let mut rig = Rig::new();
    rig.take_reference(&frame_of(flat(0.4), 1.0), 1.0);
    let other = frame_of(flat(0.03), 1.0);
    // Первый кадр смены — изменилась половина кадра, второй — весь; подтверждением уйдёт последнее свидетельство.
    assert_eq!(
        rig.watch(&frame_of(halves(0.4, 0.03), 1.0), 1.0),
        SceneWatchStep::Diverging
    );
    assert_eq!(rig.watch(&other, 1.0), SceneWatchStep::Diverging);
    // Зум на третьем кадре смены (подтверждения по времени ещё нет).
    rig.framing = PixelRect {
        left: 1000,
        top: 750,
        size: PixelSize::new(2000, 1500),
    };
    let step = rig.watch(&other, 1.0);
    assert_eq!(
        step,
        SceneWatchStep::Confirmed(ChangeEvidence {
            changed_cells: 48,
            voting_cells: 48
        })
    );
    // Эталон переснят с зумированного кадра: дальше «похоже».
    assert_eq!(rig.watch(&other, 1.0), SceneWatchStep::Similar);
}

#[test]
fn growing_exposure_that_starts_to_clip_cells_gives_no_false_votes() {
    // Выдержка растёт вдвое каждый кадр, а ячейки пересвечиваются постепенно (0 → 3 % → 10 % → ...): сцена та же.
    let mut rig = Rig::new();
    let cell_with_bright_spots = |exposure: f32, spot_share: f32| {
        // Яркие точки (свет 0,5 на единицу) пересвечиваются при выдержке 2 и выше; остальное — свет 0,3.
        let spots = spot_share;
        let spot_level = 0.5 * exposure;
        let base_level = 0.3 * exposure;
        let saturated = if spot_level >= 0.95 { spots } else { 0.0 };
        let unclipped_mean = (1.0 - spots) * base_level + if saturated == 0.0 { spots * spot_level } else { 0.0 };
        stats_from_tiles(move |_, _| {
            let mut tile = crate::exposure::test_support::partly_clipped_tile(
                (unclipped_mean / (1.0 - saturated)).min(0.94),
                saturated,
            );
            tile.saturated_cells = (4096.0 * saturated) as u32;
            tile
        })
    };
    rig.take_reference(&cell_with_bright_spots(1.0, 0.03), 1.0);
    let mut steps = Vec::new();
    for (exposure, spots) in [
        (1.0, 0.03),
        (1.5, 0.03),
        (2.0, 0.03),
        (2.0, 0.10),
        (2.5, 0.10),
        (3.0, 0.10),
    ] {
        steps.push(rig.watch(&cell_with_bright_spots(exposure, spots), f64::from(exposure)));
    }
    assert_never_suspicious(&steps);
}

#[test]
fn a_small_statistics_grid_still_gives_a_conclusion() {
    // 2×2 плитки: четверть подписи — одна ячейка, значит нужен один голос (а не 12 из конфига).
    let grid = TileGrid { columns: 2, rows: 2 };
    let uniform = |level: f32| AwbStatistics::from_tiles(grid, vec![tile_of([level; 3]); 4]).unwrap();
    let mut rig = Rig::new();
    rig.take_reference(&uniform(0.4), 1.0);
    assert_eq!(rig.watch(&uniform(0.4), 1.0), SceneWatchStep::Similar);
    let steps: Vec<_> = (0..8).map(|_| rig.watch(&uniform(0.03), 1.0)).collect();
    let SceneWatchStep::Confirmed(evidence) = *steps.last().unwrap() else {
        panic!("{steps:?}")
    };
    assert_eq!((evidence.changed_cells, evidence.voting_cells), (4, 4));
    // Единственная ячейка (1×1) тоже голосует.
    let single =
        |level: f32| AwbStatistics::from_tiles(TileGrid { columns: 1, rows: 1 }, vec![tile_of([level; 3])]).unwrap();
    let mut one = Rig::new();
    one.take_reference(&single(0.4), 1.0);
    assert_eq!(one.watch(&single(0.4), 1.0), SceneWatchStep::Similar);
}

#[test]
fn a_reference_cell_that_contradicts_its_bound_is_not_refined_by_the_current_frame() {
    // Эталон: угол A окно (пересвет), остальное 0,4. На кадре угол A измерен 0,2 — темнее границы: это изменение, а не
    // раскрытие, поэтому ячейка остаётся границей и продолжает голосовать «изменилась».
    let corner = |column: u32, row: u32| column < 8 && row < 16;
    let extra = |column: u32, row: u32| column >= 16 && row < 12;
    let reference = |column: u32, row: u32| if corner(column, row) { 5.0 } else { 0.4 };
    let mut rig = Rig::new();
    rig.take_reference(&frame_of(reference, 1.0), 1.0);
    // 8 ячеек из 48 изменились: доля 0,17 — «похож»; ячейки угла не имеют права стать новым эталоном.
    let corner_darker = |column: u32, row: u32| if corner(column, row) { 0.2 } else { 0.4 };
    assert_eq!(rig.watch(&frame_of(corner_darker, 1.0), 1.0), SceneWatchStep::Similar);
    // Ещё 12 ячеек потемнели: вместе с углом 20 из 48 = 0,42 ≥ 0,4. Если бы угол стал измеренным, их было бы 12 (0,25).
    let more_changes = |column: u32, row: u32| {
        if extra(column, row) {
            0.4 * (-1.5_f32).exp2()
        } else {
            corner_darker(column, row)
        }
    };
    assert_eq!(rig.watch(&frame_of(more_changes, 1.0), 1.0), SceneWatchStep::Diverging);
}
