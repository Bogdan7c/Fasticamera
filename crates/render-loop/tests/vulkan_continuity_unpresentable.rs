//! Поток камеры стримит, но кадры нельзя показать (нет параметров ISP): удержание не кончается чёрным
//! «камера недоступна» и не длится вечно (сессия 13, решения 110 и 113). Настоящий бэкенд показа на RADV
//! (headless, полная validation), стенд — `continuity_rig`.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod continuity_rig;
mod support;
mod vulkan_support;

use std::sync::Arc;

use continuity_rig::{
    MAIN, MAX_TICKS, Rig, SIZE_A, SIZE_B, WIDE, assert_picture, content_size, finish, gray, passport, reddish,
    reference_color, test_continuity,
};
use render_loop::config::RevealExposureCheck;
use render_loop::viewfinder::{BlackKind, FrameLabel, TransitionKind};
use ui_draw::frame_source::BackdropContinuity;

/// Новый поток шлёт кадры без метаданных съёмки: параметров ISP нет, ни один кадр не показать. Раньше такие
/// кадры не считались жизнью потока, и через `max_hold_without_stream_ms` (60 мс) удержание кончалось чёрным
/// «камера недоступна», хотя камера стримит. Теперь они откладывают «потока нет», а чёрный наступает по
/// `max_hold_ms` (25 мс) и называется честно: «показать нечего». Когда метаданные появились, картинка вернулась.
#[test]
fn a_streaming_camera_without_a_presentable_frame_ends_the_hold_as_picture_unavailable_and_recovers() {
    let Some(mut rig) = Rig::new(test_continuity(RevealExposureCheck::Ignore), false) else {
        return;
    };
    let passport_a = Arc::new(passport(MAIN, SIZE_A));
    let passport_b = Arc::new(passport(WIDE, SIZE_B));
    let blue_ref = reference_color(&passport_b, reddish());

    let epoch_a = rig.begin(&passport_a);
    rig.feed_until_live(epoch_a, gray());
    rig.stream.producer().end_stream(epoch_a);
    let first_hold = rig.tick();
    assert_eq!(
        first_hold.record.label,
        FrameLabel::Transition(TransitionKind::HeldLastFrame)
    );

    // B стримит кадр за кадром, но без метаданных: ждём чёрного.
    let epoch_b = rig.begin(&passport_b);
    let mut ticks = Vec::new();
    for _ in 0..MAX_TICKS {
        rig.offer_without_metadata(epoch_b, reddish());
        let tick = rig.tick();
        let black = matches!(tick.record.label, FrameLabel::Black(_));
        ticks.push(tick);
        if black {
            break;
        }
    }
    let last = ticks.last().expect("тики были");
    assert_eq!(
        last.record.label,
        FrameLabel::Black(BlackKind::PictureUnavailable),
        "камера стримит, показать нечего: свой честный чёрный, а не «камера недоступна»"
    );
    assert!(
        ticks[..ticks.len() - 1]
            .iter()
            .all(|tick| tick.record.label == FrameLabel::Transition(TransitionKind::HeldLastFrame)),
        "до чёрного только удержанный кадр"
    );
    assert_eq!(last.ui_backdrop, BackdropContinuity::NoCameraFrame);
    assert!(!last.holding);
    let counters = rig.counters();
    assert_eq!(counters.holds_ended_without_presentable_frame, 1, "{counters:?}");
    assert_eq!(counters.holds_ended_without_stream, 0, "{counters:?}");
    assert_eq!(counters.black_frames_camera_unavailable, 0, "{counters:?}");
    assert_eq!(counters.black_frames_while_picture_expected, 0, "{counters:?}");
    assert!(
        counters.last_hold_duration_ms.is_some_and(|ms| (25..60).contains(&ms)),
        "удержание кончилось по max_hold_ms (25), а не по «потока нет» (60): {counters:?}"
    );

    // Метаданные появились: первая же готовая картинка возвращает живой экран.
    let recovery = rig.feed_until_live(epoch_b, reddish());
    assert_eq!(recovery.last().map(|tick| tick.record.label), Some(FrameLabel::Live));
    assert_picture(&rig.live(), content_size(SIZE_B), blue_ref, "B после восстановления");
    finish(&mut rig, &[epoch_b]);
}
