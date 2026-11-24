//! Диагностика белой на потоке рендера (white-balance 01): какая белая применена, что просил владелец, чем
//! ответил источник, как прошёл холодный старт. Старый оценщик (`last_estimate`, `estimates`, статистика) и
//! белая HAL (`hal_white`) — только диагностика: в картинку они не идут (удалит сессия 03 плана).
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

/// Последняя оценка света.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EstimateReading {
    /// Оценённая белая (после привязки к полосе вокруг кривой Планка).
    pub white: WhiteReading,
    /// Уверенность `[0, 1]`.
    pub confidence: f32,
    /// Принятые серые плитки.
    pub accepted_tiles: u32,
    /// Оценку пришлось прижать к полосе вокруг кривой Планка.
    pub clamped_to_band: bool,
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
    /// Белая, которой сейчас считается ISP. `None` — указаний не было (временная D65).
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
    /// Последняя оценка света старым оценщиком — ТОЛЬКО диагностика, на картинку не влияет.
    pub last_estimate: Option<EstimateReading>,
    /// Белая по подсказкам HAL (`white_of_hints`) — для сравнения.
    pub hal_white: Option<WhiteReading>,
    /// Применённая белая минус белая HAL.
    pub hal_delta: Option<WhiteDelta>,
    /// Сколько оценок света получено от старого оценщика (любого исхода).
    pub estimates: u64,
    /// Сколько оценок не удалось (ошибка перевода, отказ исполнителя).
    pub estimation_failures: u64,
    /// Сколько раз исполнитель оценки был занят и кадр статистики пропущен.
    pub estimator_busy: u64,
    /// Каждый N-й обработанный кадр считает статистику (конфиг).
    pub statistics_every_n_frames: u32,
    /// Сколько кадров попросили статистику.
    pub statistics_requested: u64,
    /// Сколько кадров пропустили статистику из-за прореживания.
    pub statistics_skipped_by_decimation: u64,
    /// Сколько статистик пришло от потока, которого уже нет (отброшены).
    pub statistics_stale: u64,
    /// Состояние холодного старта.
    pub cold_start: ColdStartState,
    /// Сколько кадров скрыто на холодных стартах за сессию.
    pub hidden_frames_cold_start: u64,
    /// Сколько раз скрытие кончилось принудительным показом (предел исчерпан без ответа).
    pub cold_start_forced: u64,
    /// Сколько кадров обработано с временной белой (указаний не было).
    pub frames_with_provisional_white: u64,
    /// Таблицу локуса потока построить не удалось (текст причины): старый оценщик не работает.
    pub table_failure: Option<String>,
    /// Сколько оценок света светодиода получено по кадрам с фонариком предвспышки (14d).
    pub torch_white_estimates: u64,
    /// Сколько кадров с фонариком оценки не дали (нет серых плиток, ошибка, исполнитель занят).
    pub torch_white_failures: u64,
}

impl WhiteBalanceDiagnostics {
    /// Диагностика без сессии показа: ничего ещё не происходило.
    pub fn idle(statistics_every_n_frames: u32) -> Self {
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
            last_estimate: None,
            hal_white: None,
            hal_delta: None,
            estimates: 0,
            estimation_failures: 0,
            estimator_busy: 0,
            statistics_every_n_frames,
            statistics_requested: 0,
            statistics_skipped_by_decimation: 0,
            statistics_stale: 0,
            cold_start: ColdStartState::Warm,
            hidden_frames_cold_start: 0,
            cold_start_forced: 0,
            frames_with_provisional_white: 0,
            table_failure: None,
            torch_white_estimates: 0,
            torch_white_failures: 0,
        }
    }
}
