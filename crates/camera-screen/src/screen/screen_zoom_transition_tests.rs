//! Завеса перехода между камерами (сессия 13, решения 110, 112, 113): размытие удержанного кадра, его растворение
//! и поворот-карточка при перевороте. Рендер сообщает `BackdropContinuity`, экран рисует размытие стеклом.

use camera_contract::ids::LensRole;
use shell_contract::clock::MonotonicNanos;
use ui_core::time::MotionPreference;
use ui_draw::color::DisplayColor;
use ui_draw::draw_list::{DrawItem, DrawList, GlassGroupItem};
use ui_draw::frame_source::BackdropContinuity;
use ui_draw::shape::ShapeGeometry;

use super::test_rig::{FRAME_MS, PX_PER_PT, Rig, ms, near, px};

/// Размытие кадра: группа стекла с радиусом размытия пресета `frost` (28 pt).
fn frost_groups(list: &DrawList) -> Vec<&GlassGroupItem> {
    list.items
        .iter()
        .filter_map(|item| match item {
            DrawItem::Glass(group) if near(group.params.blur_radius_px, 28.0 * PX_PER_PT, 0.5) => Some(group),
            _ => None,
        })
        .collect()
}

fn frost_opacity(list: &DrawList) -> f32 {
    frost_groups(list).first().map_or(0.0, |group| group.opacity.get())
}

/// Ширина окна размытия, pt (0 — окна нет).
fn frost_width_pt(list: &DrawList) -> f64 {
    frost_groups(list)
        .first()
        .and_then(|group| group.members.first())
        .map_or(0.0, |member| match member.geometry {
            ShapeGeometry::RoundedRect { size, .. } => f64::from(size.width) / f64::from(PX_PER_PT),
            _ => 0.0,
        })
}

/// Есть ли чёрная подложка на всю площадь кадра (фон «пустоты» вокруг карточки).
fn black_card_background(rig: &Rig, list: &DrawList) -> bool {
    let frame = rig.rect_with(|geometry| geometry.frame(rig.screen.state().settings().aspect));
    list.items.iter().any(|item| match item {
        DrawItem::Shape(shape) => {
            shape.fill == DisplayColor::BLACK
                && shape.opacity.get() >= 0.99
                && matches!(shape.geometry, ShapeGeometry::RoundedRect { size, .. }
                    if near(size.width, px(frame.size.width), 1.0) && near(size.height, px(frame.size.height), 1.0))
        }
        _ => false,
    })
}

fn at(rig: &mut Rig, absolute_ms: i64) -> DrawList {
    rig.now_ms = absolute_ms;
    rig.frame().clone()
}

#[test]
fn a_held_frame_blurs_the_viewfinder_within_one_frame_and_keeps_it_blurred() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    assert!(frost_groups(rig.run(32)).is_empty(), "живой кадр — без размытия");

    rig.backdrop = BackdropContinuity::HeldLastFrame { since: ms(rig.now_ms) };
    let first = rig.run(FRAME_MS).clone();
    assert_eq!(frost_groups(&first).len(), 1, "размытие включилось за один кадр");
    assert!(near(frost_opacity(&first), 1.0, 1.0e-6));
    // Окно размытия закрывает весь кадр видоискателя.
    let frame = rig.rect_with(|geometry| geometry.frame(rig.screen.state().settings().aspect));
    assert!(near(frost_width_pt(&first) as f32, frame.size.width as f32, 0.5));
    // Пока кадр удержан, размытие не слабеет.
    let later = rig.run(1000).clone();
    assert!(near(frost_opacity(&later), 1.0, 1.0e-6));
    // Чёрного фона у обычной смены объектива нет (только у переворота).
    assert!(!black_card_background(&rig, &later));
}

#[test]
fn when_the_live_frame_returns_the_blur_dissolves_in_two_tenths_of_a_second() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    rig.backdrop = BackdropContinuity::HeldLastFrame { since: ms(0) };
    rig.run(64);
    rig.backdrop = BackdropContinuity::Live;
    let start = rig.now_ms + FRAME_MS;
    let first = at(&mut rig, start);
    assert!(
        near(frost_opacity(&first), 1.0, 1.0e-6),
        "растворение начинается с полного размытия"
    );
    let middle = at(&mut rig, start + 100);
    assert!(near(frost_opacity(&middle), 0.5, 0.05), "{}", frost_opacity(&middle));
    let almost = at(&mut rig, start + 190);
    assert!(frost_opacity(&almost) < 0.1 && frost_opacity(&almost) > 0.0);
    let gone = at(&mut rig, start + 200);
    assert!(frost_groups(&gone).is_empty(), "через 0,2 с размытия нет в списке");
}

#[test]
fn a_new_hold_during_the_dissolve_blurs_fully_again() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    rig.backdrop = BackdropContinuity::HeldLastFrame { since: ms(0) };
    rig.run(64);
    rig.backdrop = BackdropContinuity::Live;
    let start = rig.now_ms + FRAME_MS;
    at(&mut rig, start);
    at(&mut rig, start + 100);
    rig.backdrop = BackdropContinuity::HeldLastFrame { since: ms(start + 100) };
    let held_again = at(&mut rig, start + 116);
    assert!(near(frost_opacity(&held_again), 1.0, 1.0e-6));
}

#[test]
fn no_camera_frame_needs_no_blur() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    rig.backdrop = BackdropContinuity::NoCameraFrame;
    assert!(frost_groups(rig.run(64)).is_empty());
}

// ---- переворот ----

fn flip_at(rig: &mut Rig) -> i64 {
    let center = rig.center_with(|geometry| geometry.flip_button());
    rig.tap(center);
    let mark = rig.screen.state().zoom().last_flip().expect("переворот запущен");
    let started = mark.at.as_nanos() / 1_000_000;
    // Рендер сразу сообщил об удержании кадра.
    rig.backdrop = BackdropContinuity::HeldLastFrame {
        since: MonotonicNanos::from_nanos(mark.at.as_nanos()),
    };
    started
}

#[test]
fn the_flip_turns_the_blurred_frame_as_a_card_over_black_instead_of_a_black_pause() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    rig.run(32);
    let started = flip_at(&mut rig);
    let frame = rig.rect_with(|geometry| geometry.frame(rig.screen.state().settings().aspect));

    // Начало оборота: карточка почти во всю ширину, вокруг чёрный фон (кадра «пустоты» нет — всегда есть картинка).
    let early = at(&mut rig, started + 16);
    assert!(black_card_background(&rig, &early));
    assert!(
        frost_width_pt(&early) > frame.size.width * 0.95,
        "{}",
        frost_width_pt(&early)
    );

    // Четверть оборота: заметно уже, но не нулевая ширина.
    let quarter = at(&mut rig, started + 80);
    let width_quarter = frost_width_pt(&quarter);
    assert!(
        width_quarter > 0.0 && width_quarter < frame.size.width * 0.9,
        "{width_quarter}"
    );

    // Ровно 90° (середина оборота): ширина нулевая, преобразование вырождено — окно не рисуется, а список
    // остаётся валидным (рендер отверг бы вырожденное преобразование целиком; `produce_frame` проверил список).
    let edge_on = at(&mut rig, started + 135);
    assert!(frost_groups(&edge_on).is_empty(), "на ребре карточки окна размытия нет");
    assert!(black_card_background(&rig, &edge_on), "остаётся чёрный фон");
    assert!(edge_on.validate().is_ok());

    // Вторая половина: карточка разворачивается обратно.
    let three_quarters = at(&mut rig, started + 200);
    assert!(frost_width_pt(&three_quarters) > width_quarter);

    // Оборот закончился, кадр всё ещё удержан: размытие на весь кадр, чёрного нет.
    let done = at(&mut rig, started + 300);
    assert!(near(frost_width_pt(&done) as f32, frame.size.width as f32, 0.5));
    assert!(!black_card_background(&rig, &done));
}

#[test]
fn after_the_flip_the_blur_dissolves_faster_than_after_a_lens_switch() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    rig.run(32);
    let started = flip_at(&mut rig);
    at(&mut rig, started + 300);
    rig.backdrop = BackdropContinuity::Live;
    let first = at(&mut rig, started + 316);
    assert!(near(frost_opacity(&first), 1.0, 1.0e-6));
    let half = at(&mut rig, started + 316 + 75);
    assert!(near(frost_opacity(&half), 0.5, 0.06), "{}", frost_opacity(&half));
    let gone = at(&mut rig, started + 316 + 150);
    assert!(frost_groups(&gone).is_empty());
}

#[test]
fn reduce_motion_flip_has_no_card_and_no_black_only_the_blur_and_its_dissolve() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Reduced);
    rig.run(32);
    let started = flip_at(&mut rig);
    let frame = rig.rect_with(|geometry| geometry.frame(rig.screen.state().settings().aspect));
    for offset in [16, 80, 135, 200] {
        let list = at(&mut rig, started + offset);
        assert!(!black_card_background(&rig, &list), "без чёрного, смещение {offset}");
        assert!(
            near(frost_width_pt(&list) as f32, frame.size.width as f32, 0.5),
            "карточка не сужается"
        );
    }
    rig.backdrop = BackdropContinuity::Live;
    at(&mut rig, started + 216);
    let half = at(&mut rig, started + 216 + 75);
    assert!(near(frost_opacity(&half), 0.5, 0.06));
}

#[test]
fn a_later_flip_runs_its_own_card() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    rig.run(32);
    let first = flip_at(&mut rig);
    at(&mut rig, first + 600);
    rig.backdrop = BackdropContinuity::Live;
    rig.run(400);
    let second = flip_at(&mut rig);
    assert!(second > first);
    let list = at(&mut rig, second + 80);
    assert!(
        black_card_background(&rig, &list),
        "второй переворот снова крутит карточку"
    );
}
