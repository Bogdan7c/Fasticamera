//! Сквозной тест решения 241 «точка тапа не прилипает»: НАСТОЯЩИЙ движок камеры (наш AE) + НАСТОЯЩИЙ рендер (RADV
//! headless) на общих ящиках; тест играет только камеру (стенд `tap_point_end_to_end/stand.rs`).
//!
//! Что доказывается ПИКСЕЛЯМИ и экспозицией, а не вызовами: у каждой проверки есть контрольный прогон той же
//! сцены БЕЗ тапа, и «точка отпущена» значит «яркость экрана и «выдержка × ISO» вернулись к контрольным»:
//! * тап по тёмному месту, сцена та же: точка ДЕРЖИТСЯ, экспозиция ушла вверх и стоит;
//! * сцена сменилась: без единой команды точка отпущена (`SceneChanged`), экспозиция и экран как в контроле;
//! * долгий тап (блокировка): точка и экспозиция держатся, что бы ни случилось со сценой;
//! * уход в фон и возврат: точка отпущена (`ReturnedToApp`), экспозиция матричная;
//! * политика `UntilReplaced` (прежнее поведение): точка держится и после смены сцены — контроль, что тест
//!   ловит именно решение, а не то, что AE и так «сам вернулся».

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]
#![allow(
    dead_code,
    reason = "стенды render-loop общие: каждый тестовый файл берёт свою часть"
)]

#[path = "../../render-loop/tests/ae_rig/mod.rs"]
mod ae_rig;
#[path = "../../render-loop/tests/awb_support/mod.rs"]
mod awb_support;
#[path = "../../render-loop/tests/support/mod.rs"]
mod support;
#[path = "../../render-loop/tests/vulkan_support/mod.rs"]
mod vulkan_support;
#[path = "../../render-loop/tests/white_engine/mod.rs"]
mod white_engine;

#[path = "tap_point_end_to_end/stand.rs"]
mod stand;

use ae_rig::{SceneLayout, TILE_COUNT};
use camera_contract::three_a::AeAfLock;
use camera_core::point_hold::{PointHold, PointReleaseCause};
use stand::{Reading, Stand};

/// Допуск возврата к контролю, EV (задача решения 241).
const RETURN_TOLERANCE_EV: f64 = 0.15;
/// Кадров на схождение матричной экспозиции от старта.
const MATRIX_FRAMES: usize = 120;
/// Кадров, которые точка обязана продержаться при неизменной сцене (>300, чтобы слежение успело бы сработать).
const HOLD_FRAMES: usize = 350;
/// Кадров после смены сцены или возврата: отпускание (~40 кадров) и новое схождение AE.
const SETTLE_FRAMES: usize = 250;
/// Окно усреднения итоговых показаний (кадры в конце схождения, где всё уже стоит).
const READING_WINDOW: usize = 30;
/// Насколько ярче экран при тапе (EV): тон-кривая сжимает светлое, поэтому меньше, чем рост экспозиции.
const MIN_SCREEN_GAIN_EV: f64 = 1.0;
/// Насколько экспозиция по точке обязана отличаться от матричной после смены сцены (EV): точка на светлом месте
/// опускает экспозицию на ≈ 0,8 EV (намного больше допуска возврата 0,15 EV).
const MIN_OLD_POLICY_GAP_EV: f64 = 0.5;
/// Насколько тап по тёмному месту обязан поднять экспозицию над матричной (EV): в сцене 1/8 против 1 это ≈ 2 EV.
const MIN_POINT_GAIN_EV: f64 = 1.5;

/// Левая половина кадра темнее правой в 8 раз.
fn left_dark_right_bright() -> SceneLayout {
    SceneLayout::PerTile(
        (0..TILE_COUNT)
            .map(|index| if index % 32 < 16 { 0.125 } else { 1.0 })
            .collect(),
    )
}

/// Сцена после «панорамы»: тёмное и светлое поменялись местами (на месте тапа теперь светлое).
fn left_bright_right_dark() -> SceneLayout {
    SceneLayout::PerTile(
        (0..TILE_COUNT)
            .map(|index| if index % 32 < 16 { 1.0 } else { 0.125 })
            .collect(),
    )
}

/// Как человек трогает экран.
#[derive(Clone, Copy)]
enum Touch {
    None,
    Tap,
    LongTap,
}

/// Стенд, прошедший матричное схождение и (если есть) тап, после чего `HOLD_FRAMES` кадров неизменной сцены.
/// Пока точка должна держаться, каждые 25 кадров проверяется, что она на месте и движок её не отпускал.
fn held_for_a_while(hold: PointHold, touch: Touch) -> Option<Stand> {
    let mut stand = Stand::start(hold, left_dark_right_bright())?;
    stand.resume();
    stand.run(MATRIX_FRAMES);
    match touch {
        Touch::None => stand.run(HOLD_FRAMES),
        Touch::Tap | Touch::LongTap => {
            match touch {
                Touch::Tap => stand.tap(),
                _ => stand.long_tap(),
            }
            stand.wait_point_held();
            for _ in 0..HOLD_FRAMES / 25 {
                stand.run(25);
                assert!(stand.point_is_held(), "сцена прежняя: точка держится");
                assert!(stand.released_causes().is_empty(), "точку никто не отпускал");
            }
        }
    }
    Some(stand)
}

/// Показания контрольного прогона: та же сцена и та же последовательность смен, но без единого касания.
fn control_after_scene_change() -> Option<Reading> {
    let mut control = held_for_a_while(PointHold::UntilSceneChange, Touch::None)?;
    control.change_scene(left_bright_right_dark());
    control.run(SETTLE_FRAMES);
    let reading = control.reading(READING_WINDOW);
    control.finish();
    Some(reading)
}

#[test]
fn a_tap_on_a_dark_place_is_held_while_the_scene_stays_the_same() {
    let Some(control) = held_for_a_while(PointHold::UntilSceneChange, Touch::None) else {
        return;
    };
    let matrix = control.reading(READING_WINDOW);
    control.finish();

    let tapped = held_for_a_while(PointHold::UntilSceneChange, Touch::Tap).expect("GPU есть (контроль уже поднялся)");
    let held = tapped.reading(READING_WINDOW);
    // Экспозиция ушла вверх за тёмным местом, и экран показывает результат этого: ярче, чем при матричной.
    assert!(
        held.product_ev_above(&matrix) >= MIN_POINT_GAIN_EV,
        "тап поднял экспозицию: {held:?} против матричной {matrix:?}"
    );
    assert!(
        held.luminance_ev_above(&matrix) >= MIN_SCREEN_GAIN_EV,
        "экран стал ярче вслед за экспозицией: {held:?} против {matrix:?}"
    );
    assert!(
        tapped.luminance_spread_ev(100) < 0.05,
        "яркость за последние 100 кадров стабильна: разброс {} EV",
        tapped.luminance_spread_ev(100)
    );
    assert_eq!(tapped.points_released(), 0, "точку не отпускали");
    tapped.finish();
}

#[test]
fn the_point_is_released_by_the_scene_change_alone_and_the_picture_returns_to_the_untapped_one() {
    let Some(control) = control_after_scene_change() else {
        return;
    };
    let mut stand = held_for_a_while(PointHold::UntilSceneChange, Touch::Tap).expect("GPU есть");
    let held = stand.reading(READING_WINDOW);

    // Сцена сменилась; людей и команд нет. Первые кадры (меньше времени подтверждения) точка ещё держится.
    stand.change_scene(left_bright_right_dark());
    stand.run(5);
    assert!(stand.point_is_held(), "смена сцены ещё не подтверждена: точка на месте");
    stand.run(SETTLE_FRAMES);

    assert!(stand.whole_frame(), "точка отпущена: замер снова по всему кадру");
    assert_eq!(stand.point_and_lock().1, AeAfLock::Unlocked);
    assert_eq!(stand.released_causes(), vec![PointReleaseCause::SceneChanged]);
    assert_eq!(stand.points_released(), 1, "точка отпущена ровно один раз");
    let returned = stand.reading(READING_WINDOW);
    assert!(
        returned.product_ev_above(&control).abs() <= RETURN_TOLERANCE_EV,
        "«выдержка × ISO» вернулась к контрольной: {returned:?} против {control:?}"
    );
    assert!(
        returned.luminance_ev_above(&control).abs() <= RETURN_TOLERANCE_EV,
        "яркость показанных пикселей вернулась к контрольной: {returned:?} против {control:?}"
    );
    // Защита от «вернулась сама»: на тапе экспозиция была совсем другой.
    assert!(
        held.product_ev_above(&control).abs() > MIN_POINT_GAIN_EV,
        "{held:?} {control:?}"
    );
    stand.finish();
}

#[test]
fn a_long_tap_keeps_the_point_and_the_exposure_through_a_scene_change() {
    let Some(control) = control_after_scene_change() else {
        return;
    };
    let mut stand = held_for_a_while(PointHold::UntilSceneChange, Touch::LongTap).expect("GPU есть");
    assert_eq!(stand.point_and_lock().1, AeAfLock::Locked, "долгий тап блокирует AE/AF");
    let before = stand.reading(READING_WINDOW);

    stand.change_scene(left_bright_right_dark());
    stand.run(SETTLE_FRAMES);

    assert!(stand.point_is_held(), "блокировка держит точку");
    assert_eq!(stand.point_and_lock().1, AeAfLock::Locked);
    assert!(stand.released_causes().is_empty(), "события отпускания нет");
    let after = stand.reading(READING_WINDOW);
    assert!(
        after.product_ev_above(&before).abs() <= 0.05,
        "экспозиция заморожена: {after:?} против {before:?}"
    );
    assert!(
        after.product_ev_above(&control).abs() > MIN_POINT_GAIN_EV,
        "заблокированная экспозиция не вернулась к матричной: {after:?} против {control:?}"
    );
    stand.finish();
}

#[test]
fn returning_from_the_background_releases_the_point_and_the_exposure_goes_back_to_matrix() {
    // Контроль: та же сцена, то же «фон и возврат», но без тапа.
    let Some(mut control) = held_for_a_while(PointHold::UntilSceneChange, Touch::None) else {
        return;
    };
    control.pause();
    control.resume();
    control.run(SETTLE_FRAMES);
    let matrix = control.reading(READING_WINDOW);
    control.finish();

    let mut stand = held_for_a_while(PointHold::UntilSceneChange, Touch::Tap).expect("GPU есть");
    let held = stand.reading(READING_WINDOW);
    assert!(
        held.product_ev_above(&matrix) >= MIN_POINT_GAIN_EV,
        "{held:?} {matrix:?}"
    );
    stand.pause();
    stand.resume();
    assert!(stand.whole_frame(), "возврат из фона сразу снимает точку");
    assert_eq!(stand.released_causes(), vec![PointReleaseCause::ReturnedToApp]);
    stand.run(SETTLE_FRAMES);

    let returned = stand.reading(READING_WINDOW);
    assert!(
        returned.product_ev_above(&matrix).abs() <= RETURN_TOLERANCE_EV,
        "матричная экспозиция: {returned:?} против {matrix:?}"
    );
    assert!(
        returned.luminance_ev_above(&matrix).abs() <= RETURN_TOLERANCE_EV,
        "яркость экрана как в контроле: {returned:?} против {matrix:?}"
    );
    stand.finish();
}

#[test]
fn with_the_old_policy_the_point_survives_a_scene_change() {
    let Some(control) = control_after_scene_change() else {
        return;
    };
    let mut stand = held_for_a_while(PointHold::UntilReplaced, Touch::Tap).expect("GPU есть");
    stand.change_scene(left_bright_right_dark());
    stand.run(SETTLE_FRAMES);

    assert!(stand.point_is_held(), "прежняя политика: точка живёт до нового тапа");
    assert!(stand.released_causes().is_empty());
    let kept = stand.reading(READING_WINDOW);
    assert!(
        kept.product_ev_above(&control) < -MIN_OLD_POLICY_GAP_EV,
        "экспозиция всё ещё следует за точкой, а не за кадром: {kept:?} против {control:?}"
    );
    assert!(
        kept.luminance_ev_above(&control) < -MIN_OLD_POLICY_GAP_EV,
        "и экран не вернулся к контрольному: {kept:?} против {control:?}"
    );
    stand.finish();
}
