//! Применение состояния работы к рендеру и платформе (сессия 16a): единственное место, где `viewfinder-stage-budget`
//! шлёт команды рычагов.
//!
//! Порядок команд состояния фиксирован и проверяется тестами: рисовальщик → стадии ISP → маска подавления (только
//! если строка вообще её трогает) → запрос частоты экрана (только у строки с частотой). Старые строки команду
//! подавления и частоты не шлют, поэтому их поток команд в рендер прежний.
//!
//! Возврат рычагов к продукту по концу сценария делает маршрутизатор (`render_scenario_ended`); здесь только
//! [`restore_product_levers`] — возврат ДО конца, чтобы жест выхода с экрана калибровки шёл при продуктовом
//! состоянии, а не при подавленном UI.

use shell_contract::commands::FrameRateOutcome;

use super::screen_gesture::Gesture;
use super::stage_budget_states::{RowStates, WorkState};
use crate::scenario::observing::{ScenarioContext, stage_fields};
use crate::screen_scenario_config::{HoldGesture, ScreenPoint};

/// Имя стадии удерживаемого касания (контракт с `scripts/smoke_driver.py`).
pub const HOLD_TOUCH_STAGE: &str = "budget_hold_touch";

/// Какие рычаги сценарий уже трогал за прогон. Состояние окна не должно зависеть от того, какая строка шла до него:
/// рычаг, который тронула прежняя строка, текущая обязана вернуть к значению состояния, даже если сама его не трогает.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LeverHistory {
    /// Хоть раз отправлялась маска подавления.
    pub suppression: bool,
    /// Хоть раз запрашивалась частота экрана.
    pub display: bool,
}

impl LeverHistory {
    /// Учесть, что состояние `state` строки `states` применено.
    pub fn note_applied(&mut self, states: &RowStates, state: &WorkState) {
        self.suppression |= states.drives_suppression();
        self.display |= state.display.target_hz().is_some();
    }
}

/// Применяет ПОЛНОЕ состояние `state` строки `states`: рисовальщик, стадии, маску (если строка или прежние строки её
/// трогали) и частоту (так же; состояние «не трогать» после строки с частотой возвращает частоту продукта из конфига
/// платформы). Недоставленное дописывается в `undelivered` с пометкой `label`.
pub fn apply_work_state(
    context: &ScenarioContext<'_>,
    states: &RowStates,
    history: LeverHistory,
    state: &WorkState,
    label: &str,
    undelivered: &mut Vec<String>,
) {
    let mut record = |what: &str, delivered: bool, detail: String| {
        if !delivered {
            undelivered.push(format!("{label}: {what}: {detail}"));
        }
    };
    let painter = context.gpu.set_viewfinder_painter_for_scenario(state.painter);
    record("рисовальщик", painter.is_delivered(), format!("{painter:?}"));
    let stages = context.gpu.set_isp_stages_for_scenario(state.stages);
    record("стадии ISP", stages.is_delivered(), format!("{stages:?}"));
    if states.drives_suppression() || history.suppression {
        let suppression = context.gpu.suppress_gpu_work_for_scenario(state.suppression);
        record(
            "подавление работы GPU",
            suppression.is_delivered(),
            format!("{suppression:?}"),
        );
    }
    // Строка без частоты после строки с частотой возвращает частоту продукта (она же — возврат маршрутизатора).
    let requested_hz = state
        .display
        .target_hz()
        .or_else(|| history.display.then_some(context.config.preferred_frame_rate_hz));
    if let Some(hertz) = requested_hz {
        match context
            .gpu
            .request_display_refresh_for_scenario(context.services.commands.as_ref(), hertz)
        {
            // «Отложено до surface» — тоже принято: пожелание применится к следующей surface.
            Ok(FrameRateOutcome::Applied | FrameRateOutcome::DeferredUntilSurface) => {}
            Ok(outcome @ FrameRateOutcome::Rejected { .. }) => record("частота экрана", false, format!("{outcome:?}")),
            Err(error) => record("частота экрана", false, format!("{hertz} Гц: {error}")),
        }
    }
}

/// Возвращает продуктовые рычаги, не дожидаясь конца сценария (перед жестом выхода с экрана калибровки).
pub fn restore_product_levers(context: &ScenarioContext<'_>, drove_suppression: bool, undelivered: &mut Vec<String>) {
    let product = WorkState::PRODUCT;
    let painter = context.gpu.set_viewfinder_painter_for_scenario(product.painter);
    if !painter.is_delivered() {
        undelivered.push(format!("возврат рисовальщика: {painter:?}"));
    }
    let stages = context.gpu.set_isp_stages_for_scenario(product.stages);
    if !stages.is_delivered() {
        undelivered.push(format!("возврат стадий ISP: {stages:?}"));
    }
    if drove_suppression {
        let suppression = context.gpu.suppress_gpu_work_for_scenario(product.suppression);
        if !suppression.is_delivered() {
            undelivered.push(format!("возврат подавления работы GPU: {suppression:?}"));
        }
    }
}

/// Начинать ли новое нажатие в этом окне.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldPress {
    /// Палец ещё не на экране: драйвер запускает нажатие.
    Start,
    /// Нажатие предыдущего окна A ещё держится и покрывает и это окно: драйвер ничего не делает (второе нажатие поверх
    /// первого оборвало бы его раньше времени).
    ContinuesPrevious,
}

/// Просьба драйверу удерживать палец.
#[derive(Debug, Clone, Copy)]
pub struct HoldTouch<'row> {
    pub row: &'row str,
    /// Номер окна A, в начале которого выдаётся стадия.
    pub window: usize,
    pub point: ScreenPoint,
    /// Сколько мс держать палец (для `Start`; для `ContinuesPrevious` — справка о том, что уже держится).
    pub hold_ms: u64,
    /// На сколько окон подряд хватает этого нажатия.
    pub windows_covered: usize,
    pub press: HoldPress,
}

/// Выдаёт стадию «удерживай палец»: драйвер выполняет её ФОНОВЫМ процессом `adb` (минуту синхронного `input swipe`
/// драйвер ждать не может: он читает logcat и обязан увидеть следующие стадии). Стадия выдаётся в начале КАЖДОГО окна A;
/// соседние окна A при ABBA идут подряд, и одно длинное нажатие покрывает оба ([`HoldPress::ContinuesPrevious`]).
pub fn emit_hold_touch(context: &mut ScenarioContext<'_>, hold: HoldTouch<'_>) {
    context.emit_stage(
        HOLD_TOUCH_STAGE,
        stage_fields([
            ("row", hold.row.into()),
            ("window", hold.window.into()),
            ("needs_new_press", (hold.press == HoldPress::Start).into()),
            ("windows_covered", hold.windows_covered.into()),
            (
                "gesture",
                Gesture::long_press(HoldGesture {
                    at: hold.point,
                    hold_ms: hold.hold_ms,
                })
                .to_stage_value(),
            ),
        ]),
    );
}
