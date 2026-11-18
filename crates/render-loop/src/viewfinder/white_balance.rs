//! Диагностика нашего AWB на потоке рендера (сессия 08): что решил, чем руководствовался, сколько стоило.
//!
//! Приходит в `RenderStatus` внутри [`super::ViewfinderDiagnostics`]. Белая HAL здесь ТОЛЬКО для сравнения
//! (`hal_white`, `hal_delta`): в ISP она не попадает никогда (решение 62, критерий выхода 08).

use camera_contract::three_a::WhiteBalanceDirective;
use scene_estimation::controller::{ControllerCounters, ManualLimit};

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

/// Наша белая минус белая HAL.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WhiteDelta {
    /// Разность по майредам (плюс — наша белая теплее).
    pub mired: f64,
    /// Разность по Duv.
    pub duv: f64,
}

/// Состояние холодного старта текущего потока.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColdStartState {
    /// Белая сцены известна: кадры показываются сразу.
    Warm,
    /// Ждём первую оценку; `hidden` кадров уже скрыто.
    Pending {
        /// Сколько кадров этого потока скрыто.
        hidden: u32,
    },
}

/// Диагностика AWB. Счётчики накопительные за сессию показа.
#[derive(Debug, Clone, PartialEq)]
pub struct WhiteBalanceDiagnostics {
    /// Действующее указание (до первой публикации — `WhiteBalanceDirective::DEFAULT`).
    pub directive: WhiteBalanceDirective,
    /// Поколение применённого указания (`None` — движок камеры ещё ничего не публиковал).
    pub directive_generation: Option<u64>,
    /// Белая, которой сейчас считается ISP (`None` — белой сцены ещё нет).
    pub scene_white: Option<WhiteReading>,
    /// Последняя оценка света (в том числе слабая, не принятая контроллером).
    pub last_estimate: Option<EstimateReading>,
    /// Ручная точка вне таблицы камеры: что просили и что применено.
    pub manual_limit: Option<ManualLimit>,
    /// Белая по коэффициентам HAL — только для сравнения.
    pub hal_white: Option<WhiteReading>,
    /// Наша белая минус белая HAL.
    pub hal_delta: Option<WhiteDelta>,
    /// Сколько оценок света получено от исполнителя (любого исхода).
    pub estimates: u64,
    /// Решения контроллера по оценкам.
    pub decisions: ControllerCounters,
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
    /// Состояние холодного старта текущего потока.
    pub cold_start: ColdStartState,
    /// Сколько кадров скрыто на холодных стартах за сессию (`hidden_frames_cold_start`).
    pub hidden_frames_cold_start: u64,
    /// Сколько раз скрытие кончилось принудительным показом (лимит исчерпан без оценки).
    pub cold_start_forced: u64,
    /// Сколько кадров обработано с временной белой (оценки ещё не было).
    pub frames_with_provisional_white: u64,
    /// Таблицу локуса потока построить не удалось (текст причины): AWB не работает, белая — временная.
    pub table_failure: Option<String>,
}

impl WhiteBalanceDiagnostics {
    /// Диагностика без сессии показа: ничего ещё не происходило.
    pub fn idle(statistics_every_n_frames: u32) -> Self {
        Self {
            directive: WhiteBalanceDirective::DEFAULT,
            directive_generation: None,
            scene_white: None,
            last_estimate: None,
            manual_limit: None,
            hal_white: None,
            hal_delta: None,
            estimates: 0,
            decisions: ControllerCounters::default(),
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
        }
    }
}
