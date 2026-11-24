//! Холодный старт белой (решения 65, 76; white-balance 01): кадры обрабатываются, но не показываются, пока
//! действует заявка источнику с `HideFrames` и предел `max_hidden_frames` не исчерпан.
//!
//! # Что такое «открытие»
//! Счёт ведётся на ОТКРЫТИЕ — непрерывный период, пока в указании стоит заявка `HideFrames`:
//! * смена потока (линза, камера) счёт не обнуляет: заявка та же, ждём тот же ответ;
//! * переизданная заявка `HideFrames` (новый номер) при уже показанных кадрах снова не прячет: картинка,
//!   однажды показанная в этом открытии, не гаснет;
//! * новое открытие начинается, когда `HideFrames` появляется после периода без него (заявка закрыта ответом,
//!   отменена, заменена `KeepShowing`, указания не было). Пауза приложения даёт новую сессию показа (новый
//!   раннер) либо новую заявку после периода без неё, поэтому прямого признака паузы не нужно.
//!
//! Заявка закрылась (ответ принят владельцем) — показ. Предел исчерпан — принудительный показ с текущей
//! белой (она всегда есть: прежняя, а если не было — дневной свет из конфига); заявка при этом живёт, и
//! поздний ответ владелец применит плавно.

use camera_contract::white::WhileAwaitingWhite;

use crate::viewfinder::ColdStartState;

/// Что делать с кадром.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColdStartVerdict {
    /// Показать: ждать нечего.
    Show,
    /// Скрыть: обработать, чтобы шла статистика, но на экран не выводить.
    Hide,
    /// Показать принудительно: предел скрытых кадров исчерпан, а ответа нет.
    ShowForced {
        /// Сколько кадров было скрыто до принудительного показа.
        hidden: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opening {
    /// Заявки с `HideFrames` нет.
    None,
    /// Открытие идёт, кадры скрыты.
    Hiding { hidden: u32 },
    /// Открытие идёт, кадры уже показаны (ответ ещё ждём).
    Revealed,
}

/// Машина состояний холодного старта.
#[derive(Debug, Clone, Copy)]
pub struct ColdStart {
    opening: Opening,
    max_hidden_frames: u32,
}

impl ColdStart {
    /// Создаётся без открытия: пока заявки нет, скрывать нечего.
    pub fn new(max_hidden_frames: u32) -> Self {
        Self {
            opening: Opening::None,
            max_hidden_frames,
        }
    }

    /// Состояние для диагностики.
    pub fn state(&self) -> ColdStartState {
        match self.opening {
            Opening::Hiding { hidden } => ColdStartState::Pending { hidden },
            Opening::None | Opening::Revealed => ColdStartState::Warm,
        }
    }

    /// Решение по очередному кадру. `awaiting` — что требует действующая заявка (`None` — заявки нет).
    pub fn decide(&mut self, awaiting: Option<WhileAwaitingWhite>) -> ColdStartVerdict {
        if awaiting != Some(WhileAwaitingWhite::HideFrames) {
            self.opening = Opening::None;
            return ColdStartVerdict::Show;
        }
        let hidden = match self.opening {
            Opening::Revealed => return ColdStartVerdict::Show,
            Opening::None => 0,
            Opening::Hiding { hidden } => hidden,
        };
        if hidden >= self.max_hidden_frames {
            self.opening = Opening::Revealed;
            return ColdStartVerdict::ShowForced { hidden };
        }
        self.opening = Opening::Hiding { hidden: hidden + 1 };
        ColdStartVerdict::Hide
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HIDE: Option<WhileAwaitingWhite> = Some(WhileAwaitingWhite::HideFrames);
    const KEEP: Option<WhileAwaitingWhite> = Some(WhileAwaitingWhite::KeepShowing);

    #[test]
    fn no_request_shows_everything() {
        let mut cold_start = ColdStart::new(10);
        assert_eq!(cold_start.decide(None), ColdStartVerdict::Show);
        assert_eq!(cold_start.decide(KEEP), ColdStartVerdict::Show);
        assert_eq!(cold_start.state(), ColdStartState::Warm);
    }

    #[test]
    fn frames_are_hidden_while_the_request_lives_and_shown_when_it_closes() {
        let mut cold_start = ColdStart::new(10);
        assert_eq!(cold_start.decide(HIDE), ColdStartVerdict::Hide);
        assert_eq!(cold_start.decide(HIDE), ColdStartVerdict::Hide);
        assert_eq!(cold_start.state(), ColdStartState::Pending { hidden: 2 });
        assert_eq!(
            cold_start.decide(None),
            ColdStartVerdict::Show,
            "заявка закрыта ответом"
        );
        assert_eq!(cold_start.state(), ColdStartState::Warm);
    }

    #[test]
    fn hiding_is_bounded_and_forced_show_keeps_showing_while_the_request_lives() {
        let mut cold_start = ColdStart::new(3);
        let verdicts: Vec<_> = (0..6).map(|_| cold_start.decide(HIDE)).collect();
        assert_eq!(
            verdicts,
            [
                ColdStartVerdict::Hide,
                ColdStartVerdict::Hide,
                ColdStartVerdict::Hide,
                ColdStartVerdict::ShowForced { hidden: 3 },
                ColdStartVerdict::Show,
                ColdStartVerdict::Show,
            ]
        );
    }

    #[test]
    fn zero_limit_never_hides() {
        let mut cold_start = ColdStart::new(0);
        assert_eq!(cold_start.decide(HIDE), ColdStartVerdict::ShowForced { hidden: 0 });
    }

    #[test]
    fn the_count_belongs_to_the_opening_so_a_reissued_request_does_not_restart_it() {
        // Переиздание заявки (новый номер, всё ещё HideFrames) выглядит для машины как тот же HIDE подряд.
        let mut cold_start = ColdStart::new(3);
        assert_eq!(cold_start.decide(HIDE), ColdStartVerdict::Hide);
        assert_eq!(cold_start.decide(HIDE), ColdStartVerdict::Hide);
        assert_eq!(cold_start.decide(HIDE), ColdStartVerdict::Hide);
        assert_eq!(cold_start.decide(HIDE), ColdStartVerdict::ShowForced { hidden: 3 });
        // Кадры уже показаны: переизданная заявка снова не прячет.
        assert_eq!(cold_start.decide(HIDE), ColdStartVerdict::Show);
    }

    #[test]
    fn a_hide_request_after_a_pause_without_one_is_a_new_opening() {
        let mut cold_start = ColdStart::new(2);
        assert_eq!(cold_start.decide(HIDE), ColdStartVerdict::Hide);
        assert_eq!(cold_start.decide(None), ColdStartVerdict::Show);
        assert_eq!(cold_start.decide(KEEP), ColdStartVerdict::Show);
        assert_eq!(
            cold_start.decide(HIDE),
            ColdStartVerdict::Hide,
            "новое открытие прячет снова"
        );
        assert_eq!(cold_start.state(), ColdStartState::Pending { hidden: 1 });
    }
}
