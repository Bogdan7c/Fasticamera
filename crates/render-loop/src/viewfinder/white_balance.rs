//! Диагностика белой на потоке рендера (white-balance 01): какая белая применена, что просил владелец, чем
//! ответил источник, как прошёл холодный старт. Белая HAL (`hal_white`) — только диагностика: в картинку она не идёт.
//! Расписание статистики кадра здесь не живёт: его диагностика — `super::statistics`.
//!
//! Приходит в `RenderStatus` внутри [`super::ViewfinderDiagnostics`].

use camera_contract::white::{WhiteAnswer, WhiteSourceRequest};

/// Белая точка как «температура + Duv».
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WhiteReading {
    /// Коррелированная температура, К.
    pub temperature_kelvin: f64,
    /// Расстояние до кривой Планка (CIE 1960 uv), плюс — к зелёному.
    pub duv: f64,
}

/// Применённая белая минус белая HAL.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WhiteDelta {
    /// Разность по майредам (плюс — наша белая теплее).
    pub mired: f64,
    /// Разность по Duv.
    pub duv: f64,
}

/// Состояние холодного старта (открытия).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColdStartState {
    /// Заявки с `HideFrames` нет или картинка уже показана: кадры показываются.
    Warm,
    /// Ждём ответ источника; `hidden` кадров уже скрыто.
    Pending {
        /// Сколько кадров этого открытия скрыто.
        hidden: u32,
    },
}

/// Диагностика белой. Счётчики накопительные за сессию показа.
#[derive(Debug, Clone, PartialEq)]
pub struct WhiteBalanceDiagnostics {
    /// Белая, которой сейчас считается ISP. `None` — указаний не было (белая стенда без движка).
    pub applied_white: Option<WhiteReading>,
    /// Цель владельца (последняя принятая белая).
    pub target_white: Option<WhiteReading>,
    /// Идёт ли переход к цели.
    pub transition_active: bool,
    /// Поколение применённого указания (`None` — владелец ничего не публиковал).
    pub directive_generation: Option<u64>,
    /// Действующая заявка источнику.
    pub request: Option<WhiteSourceRequest>,
    /// Последний опубликованный ответ источника.
    pub last_answer: Option<WhiteAnswer>,
    /// Сколько ответов опубликовано владельцу (на заявку — один).
    pub answers_published: u64,
    /// Из них толчок не дошёл: получатель ещё не привязан (ответ в ящике лежит).
    pub answers_signal_not_bound: u64,
    /// Ответов источников отброшено: не на действующую заявку или повторных.
    pub source_replies_dropped: u64,
    /// Целей, не переведённых в цветность (температура вне таблицы); белая осталась прежней.
    pub unconvertible_targets: u64,
    /// Белая по подсказкам HAL (`white_of_hints`) — для сравнения.
    pub hal_white: Option<WhiteReading>,
    /// Применённая белая минус белая HAL.
    pub hal_delta: Option<WhiteDelta>,
    /// Состояние холодного старта.
    pub cold_start: ColdStartState,
    /// Сколько кадров скрыто на холодных стартах за сессию.
    pub hidden_frames_cold_start: u64,
    /// Сколько раз скрытие кончилось принудительным показом (предел исчерпан без ответа).
    pub cold_start_forced: u64,
    /// Сколько кадров обработано без единого указания (белая стенда без движка; в продукте всегда 0).
    pub frames_without_directive: u64,
}

impl WhiteBalanceDiagnostics {
    /// Диагностика без сессии показа: ничего ещё не происходило.
    pub fn idle() -> Self {
        Self {
            applied_white: None,
            target_white: None,
            transition_active: false,
            directive_generation: None,
            request: None,
            last_answer: None,
            answers_published: 0,
            answers_signal_not_bound: 0,
            source_replies_dropped: 0,
            unconvertible_targets: 0,
            hal_white: None,
            hal_delta: None,
            cold_start: ColdStartState::Warm,
            hidden_frames_cold_start: 0,
            cold_start_forced: 0,
            frames_without_directive: 0,
        }
    }
}
