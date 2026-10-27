//! План передачи владения буферами камеры очереди Vulkan на один сабмит.
//!
//! Владение переходит между камерой (`FOREIGN`) и нашей очередью РОВНО ПО РАЗУ на кадр:
//! * **приём** — в сабмите, который первым читает новый кадр (он же ждёт acquire-fence камеры);
//! * **возврат** — в сабмите, который заменяет кадр (или рисует чёрный при остановке потока), ДО экспорта
//!   release-fence этого сабмита, поэтому fence покрывает и барьер возврата.
//!
//! ISP видоискателя (сессия 07) читает новый кадр ОДИН раз: приём и возврат — в одном сабмите
//! ([`FrameChange::Consume`], [`ReleaseNew::SameSubmit`]); дальше на экран идёт изображение
//! видоискателя, и следующий кадр экрана буфера камеры не касается.
//!
//! Перерисовка показанного кадра владение не трогает: приём из `UNDEFINED` в каждом сабмите гонялся бы с
//! чтением того же образа соседним сабмитом (между ними нет ни зависимости по исполнению, ни семафора)
//! и мог бы отбросить содержимое.

use super::lease::FrameChange;

/// Принимать ли рисуемый кадр у камеры в этом сабмите.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquireNew {
    /// Первое чтение нового кадра.
    Yes,
    /// Кадр уже наш (перерисовка) или кадра нет.
    No,
}

/// Возвращать ли камере прежний показанный кадр в этом сабмите.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseDisplayed {
    /// Сабмит заменяет или убирает показанный кадр.
    Yes,
    /// Показанный кадр остаётся.
    No,
}

/// Когда возвращать камере НОВЫЙ кадр.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseNew {
    /// В этом же сабмите, после единственного чтения (ISP видоискателя).
    SameSubmit,
    /// В сабмите, который его заменит (отладочная мозаика перерисовывает кадр).
    WhenReplaced,
    /// Нового кадра нет.
    NoNewFrame,
}

/// План барьеров владения одного сабмита.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnershipPlan {
    /// Приём нового кадра.
    pub acquire_new: AcquireNew,
    /// Возврат показанного кадра.
    pub release_displayed: ReleaseDisplayed,
    /// Возврат нового кадра.
    pub release_new: ReleaseNew,
}

impl OwnershipPlan {
    /// План по смене кадра; `has_displayed_source` — есть ли показанный кадр, который можно вернуть.
    pub fn for_change(change: FrameChange, has_displayed_source: bool) -> Self {
        let release_displayed = if has_displayed_source && change != FrameChange::Keep {
            ReleaseDisplayed::Yes
        } else {
            ReleaseDisplayed::No
        };
        let (acquire_new, release_new) = match change {
            FrameChange::Replace => (AcquireNew::Yes, ReleaseNew::WhenReplaced),
            FrameChange::Consume => (AcquireNew::Yes, ReleaseNew::SameSubmit),
            FrameChange::Keep | FrameChange::Clear => (AcquireNew::No, ReleaseNew::NoNewFrame),
        };
        Self {
            acquire_new,
            release_displayed,
            release_new,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_frame_acquires_once_and_releases_the_old_one() {
        assert_eq!(
            OwnershipPlan::for_change(FrameChange::Replace, true),
            OwnershipPlan {
                acquire_new: AcquireNew::Yes,
                release_displayed: ReleaseDisplayed::Yes,
                release_new: ReleaseNew::WhenReplaced,
            }
        );
        // Самый первый кадр: возвращать нечего.
        assert_eq!(
            OwnershipPlan::for_change(FrameChange::Replace, false),
            OwnershipPlan {
                acquire_new: AcquireNew::Yes,
                release_displayed: ReleaseDisplayed::No,
                release_new: ReleaseNew::WhenReplaced,
            }
        );
    }

    #[test]
    fn redraw_touches_no_ownership_at_all() {
        for has_displayed in [true, false] {
            assert_eq!(
                OwnershipPlan::for_change(FrameChange::Keep, has_displayed),
                OwnershipPlan {
                    acquire_new: AcquireNew::No,
                    release_displayed: ReleaseDisplayed::No,
                    release_new: ReleaseNew::NoNewFrame,
                }
            );
        }
    }

    #[test]
    fn inactive_stream_only_releases() {
        assert_eq!(
            OwnershipPlan::for_change(FrameChange::Clear, true),
            OwnershipPlan {
                acquire_new: AcquireNew::No,
                release_displayed: ReleaseDisplayed::Yes,
                release_new: ReleaseNew::NoNewFrame,
            }
        );
        assert_eq!(
            OwnershipPlan::for_change(FrameChange::Clear, false).release_displayed,
            ReleaseDisplayed::No,
            "уже чёрный: возвращать нечего"
        );
    }

    #[test]
    fn isp_frame_is_acquired_and_released_in_the_same_submit() {
        assert_eq!(
            OwnershipPlan::for_change(FrameChange::Consume, false),
            OwnershipPlan {
                acquire_new: AcquireNew::Yes,
                release_displayed: ReleaseDisplayed::No,
                release_new: ReleaseNew::SameSubmit,
            }
        );
        // Переключение с отладочной мозаики: её кадр возвращается тем же сабмитом.
        assert_eq!(
            OwnershipPlan::for_change(FrameChange::Consume, true).release_displayed,
            ReleaseDisplayed::Yes
        );
    }
}
