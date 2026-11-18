//! Холодный старт AWB (решение 65): первые кадры нового потока обрабатываются, но не показываются.
//!
//! Статистика AWB считается ДО баланса белого и читается только после fence слота, поэтому первая оценка
//! света готова не раньше чем через `frames_in_flight` показов. Пока белой сцены нет, ISP может выдать лишь
//! заведомо неверный цвет (зелёный сырой сенсор, решение 55), и показывать его нельзя.
//!
//! Правило ограничено: после `max_hidden_frames` скрытых кадров поток показывается в любом случае с лучшей
//! доступной белой (слабая оценка, а если и её нет — временная D65), причём принудительный показ считается
//! отдельно. Пустая, тёмная или пересвеченная сцена не прячет картинку вечно.

use crate::viewfinder::ColdStartState;

/// Что делать с кадром.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColdStartVerdict {
    /// Показать: белая известна (в том числе только что появилась первая оценка).
    Show,
    /// Скрыть: обработать, чтобы получить статистику, но на экран не выводить.
    Hide,
    /// Показать принудительно: лимит скрытых кадров исчерпан, а оценки так и нет.
    ShowForced {
        /// Сколько кадров было скрыто до принудительного показа.
        hidden: u32,
    },
}

/// Машина состояний холодного старта.
#[derive(Debug, Clone, Copy)]
pub struct ColdStart {
    state: ColdStartState,
    max_hidden_frames: u32,
}

impl ColdStart {
    /// Создаётся «тёплой»: до первого потока скрывать нечего.
    pub fn new(max_hidden_frames: u32) -> Self {
        Self {
            state: ColdStartState::Warm,
            max_hidden_frames,
        }
    }

    /// Начался новый поток: с известной белой сцены — сразу показ, иначе ждём первую оценку.
    pub fn begin_stream(&mut self, scene_white_known: bool) {
        self.state = if scene_white_known {
            ColdStartState::Warm
        } else {
            ColdStartState::Pending { hidden: 0 }
        };
    }

    /// Состояние (для статистики и диагностики).
    pub fn state(&self) -> ColdStartState {
        self.state
    }

    /// Решение по очередному кадру. `scene_white_known` — после шага контроллера на этом кадре.
    pub fn decide(&mut self, scene_white_known: bool) -> ColdStartVerdict {
        let ColdStartState::Pending { hidden } = self.state else {
            return ColdStartVerdict::Show;
        };
        if scene_white_known {
            self.state = ColdStartState::Warm;
            return ColdStartVerdict::Show;
        }
        if hidden >= self.max_hidden_frames {
            self.state = ColdStartState::Warm;
            return ColdStartVerdict::ShowForced { hidden };
        }
        self.state = ColdStartState::Pending { hidden: hidden + 1 };
        ColdStartVerdict::Hide
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_scene_white_shows_immediately() {
        let mut cold_start = ColdStart::new(10);
        cold_start.begin_stream(true);
        assert_eq!(cold_start.decide(true), ColdStartVerdict::Show);
        assert_eq!(cold_start.state(), ColdStartState::Warm);
    }

    #[test]
    fn frames_are_hidden_until_the_first_estimate_then_shown() {
        let mut cold_start = ColdStart::new(10);
        cold_start.begin_stream(false);
        assert_eq!(cold_start.decide(false), ColdStartVerdict::Hide);
        assert_eq!(cold_start.decide(false), ColdStartVerdict::Hide);
        assert_eq!(cold_start.state(), ColdStartState::Pending { hidden: 2 });
        assert_eq!(
            cold_start.decide(true),
            ColdStartVerdict::Show,
            "первая оценка закончила скрытие"
        );
        assert_eq!(cold_start.state(), ColdStartState::Warm);
        assert_eq!(cold_start.decide(false), ColdStartVerdict::Show, "дальше скрытия нет");
    }

    #[test]
    fn hiding_is_bounded_so_a_blank_scene_is_never_hidden_forever() {
        let mut cold_start = ColdStart::new(3);
        cold_start.begin_stream(false);
        let verdicts: Vec<_> = (0..6).map(|_| cold_start.decide(false)).collect();
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
        cold_start.begin_stream(false);
        assert_eq!(cold_start.decide(false), ColdStartVerdict::ShowForced { hidden: 0 });
    }

    #[test]
    fn new_stream_restarts_the_wait() {
        let mut cold_start = ColdStart::new(2);
        cold_start.begin_stream(false);
        assert_eq!(cold_start.decide(true), ColdStartVerdict::Show);
        cold_start.begin_stream(false);
        assert_eq!(cold_start.state(), ColdStartState::Pending { hidden: 0 });
        assert_eq!(cold_start.decide(false), ColdStartVerdict::Hide);
    }
}
