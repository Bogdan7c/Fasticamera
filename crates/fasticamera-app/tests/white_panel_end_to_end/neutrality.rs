//! Сценарий k: нейтральность и знак оттенка в пикселях. Сцена — лист ColorChecker под лампой 2800 К (кадры 0–19).
//! Белая, равная свету листа, делает серую плитку нейтральной (допуск эталона ISP), а оттенок +16 на шкале уводит
//! картинку в пурпур — туда же, куда ручка TINT «+» (`isp-model/src/look/tests/white_tests.rs`: у «+» оттенка
//! зелёный канал серой карты НИЖЕ среднего красного и синего), и не в зелёный. Стенд без вида: WARMTH и TINT нулевые.

use camera_contract::white::WhiteOrigin;
use camera_screen::WhiteAxis;

use super::session::{PHONE, WhiteSession, lamp_after};
use super::white_rig::{NEUTRAL_TOLERANCE, spread};

/// Насколько зелёный канал должен уйти от среднего красного и синего, чтобы цвет считался «явно пурпурным/зелёным».
/// 21 шаг по 2 pt: 10 pt съедает порог протяжки, остальные 32 pt = 16 единиц оттенка (0,016 Duv) — «+16» из плана.
/// Серая плитка листа почти белая: при больших оттенках (≈ 0,035) тон-кривая сжимает пересвеченные каналы к белому
/// и признак цвета пропадает, поэтому проба берётся на +16.
const STEPS: usize = 21;
const TINT_MARGIN: f32 = 0.005;

fn green_minus_purple_axis(tile: [f32; 3]) -> f32 {
    tile[1] - 0.5 * (tile[0] + tile[2])
}

#[test]
fn a_white_equal_to_the_light_of_the_sheet_makes_the_gray_tile_neutral_through_the_panel() {
    let Some(mut session) = WhiteSession::open() else {
        return;
    };
    // Лампа 2800 К названа источником при открытии: серая плитка нейтральна.
    assert!(spread(session.gray_tile_of_the_last_frame()) <= NEUTRAL_TOLERANCE);

    // Рука ставит «Lamp» 3200 К: белая не равна свету листа — плитка заметно цветная (контроль чувствительности).
    session.open_card();
    session.tap_light(3);
    session.wait_origin("рука поставила лампу", WhiteOrigin::Manual);
    session.frames(2);
    assert!(
        spread(session.gray_tile_of_the_last_frame()) > NEUTRAL_TOLERANCE * 3.0,
        "3200 К под лампой 2800 К не нейтральны: {:?}",
        session.gray_tile_of_the_last_frame()
    );

    // «Phone» отвечает светом листа: белая равна свету листа — плитка нейтральна.
    session.source.react_with(lamp_after(2, 2_800.0));
    session.tap_phone();
    session.frames(5);
    session.wait_origin("белое взято у телефона", WhiteOrigin::Source(PHONE));
    let tile = session.gray_tile_of_the_last_frame();
    assert!(spread(tile) <= NEUTRAL_TOLERANCE, "серая плитка нейтральна: {tile:?}");
    session.stand.finish();
}

/// Протянуть шкалу оттенка (палец влево — плюс, вправо — минус) и вернуть серую плитку и белое движка.
fn tint_tile_after_drag(step_pt: f64, steps: usize) -> Option<([f32; 3], f64)> {
    let mut session = WhiteSession::open()?;
    session.open_card();
    session.select_axis(WhiteAxis::Tint);
    session.drag_ruler(step_pt, steps);
    let held = session.wait_origin("шкала оттенка поставила ручное белое", WhiteOrigin::Manual);
    session.frames(5);
    let tile = session.gray_tile_of_the_last_frame();
    session.stand.finish();
    Some((tile, held.setting.tint.duv()))
}

#[test]
fn a_positive_tint_on_the_scale_pushes_the_picture_to_purple_like_the_tint_knob_plus_and_never_to_green() {
    let Some((tile, duv)) = tint_tile_after_drag(-2.0, STEPS) else {
        return;
    };
    assert!((duv - 0.016).abs() < 1e-6, "палец влево дал ровно +16 единиц: {duv}");
    assert!(
        green_minus_purple_axis(tile) < -TINT_MARGIN,
        "+Duv — пурпур: зелёный канал ниже среднего красного и синего, плитка {tile:?}"
    );
}

#[test]
fn a_negative_tint_on_the_scale_pushes_the_picture_to_green() {
    let Some((tile, duv)) = tint_tile_after_drag(2.0, STEPS) else {
        return;
    };
    assert!((duv + 0.016).abs() < 1e-6, "палец вправо дал ровно −16 единиц: {duv}");
    assert!(
        green_minus_purple_axis(tile) > TINT_MARGIN,
        "−Duv — зелёный: зелёный канал выше среднего красного и синего, плитка {tile:?}"
    );
}
