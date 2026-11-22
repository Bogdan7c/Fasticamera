//! Что видоискатель делает с кадром камеры в зависимости от того, КАКИМ запросом он снят и ЧЕМ освещён
//! (сессия 14d, вспышка).
//!
//! Два независимых вопроса о кадре:
//! 1. [`viewfinder_frame_role`] — показывать ли кадр вообще. Кадры серии (`Burst`) и кадр разового запроса
//!    со вспышкой (`FlashStill`) на экран НЕ идут: у первых разные выдержки (мигали бы), второй — снимок,
//!    а не видоискатель (вспышка на экране выглядела бы как одиночный белый кадр).
//! 2. [`frame_light`] — чем освещён кадр по его собственному результату. Кадр с фонариком предвспышки
//!    показывается, но не должен тянуть AE и AWB видоискателя к свету светодиода (`FrameLight::feeds_viewfinder_smoothing`).
//!
//! Кадр без метаданных (пара не пришла) — обычный: его происхождение неизвестно, поведение прежнее (решение 42).

use camera_contract::flash::FrameLight;
use camera_contract::frame_passport::RawFramePassport;
use camera_contract::three_a::RequestOrigin;

/// Судьба кадра в видоискателе по происхождению запроса.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ViewfinderFrameRole {
    /// Обычный кадр: идёт в ISP и на экран.
    Shown,
    /// Кадр серии с брекетингом: возвращается камере без показа.
    Bracket,
    /// Кадр разового запроса со вспышкой: возвращается камере без показа (его заберёт кольцо снимков).
    FlashStill,
}

impl ViewfinderFrameRole {
    /// Идёт ли кадр на экран.
    pub(super) fn is_shown(self) -> bool {
        self == Self::Shown
    }
}

/// Роль кадра в видоискателе по происхождению его запроса.
pub(super) fn viewfinder_frame_role(passport: &RawFramePassport) -> ViewfinderFrameRole {
    let Some(details) = passport.capture.details() else {
        return ViewfinderFrameRole::Shown;
    };
    match details.result.three_a.origin {
        RequestOrigin::Burst { .. } => ViewfinderFrameRole::Bracket,
        RequestOrigin::FlashStill { .. } => ViewfinderFrameRole::FlashStill,
        RequestOrigin::Repeating | RequestOrigin::FocusTrigger { .. } | RequestOrigin::Unknown => {
            ViewfinderFrameRole::Shown
        }
    }
}

/// Чем освещён кадр по его результату. Нет метаданных — обычный свет (подставлять нечего, прежнее поведение).
pub(super) fn frame_light(passport: &RawFramePassport) -> FrameLight {
    passport
        .capture
        .details()
        .map_or(FrameLight::Ambient, |details| details.result.three_a.flash.light())
}

#[cfg(test)]
#[path = "frame_role_tests.rs"]
mod tests;
