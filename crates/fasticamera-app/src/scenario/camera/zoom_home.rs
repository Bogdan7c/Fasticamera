//! «Дом» сценария `zoom` (сессия 13): исходное состояние Back 1× с закрытым колесом, подготовка к нему в начале прогона,
//! возврат в него после провала и суд над событиями движка, которых никто не ждал.
//!
//! * **Подготовка.** План шагов предполагает старт с задней основной камеры на 1×: тап по `.6` ждёт смену Wide→UW,
//!   переворот ждёт смену на фронталку. Если телефон стоит иначе (фронталка, ширик, зум 5× с прошлого прогона), шаги
//!   соврали бы. Поэтому исполнитель в начале сам приводит экран к дому тем же путём, что пользователь: переворот
//!   (если стоит фронталка), затем тап по кнопке `1`.
//! * **Возврат.** Сценарий не должен оставлять телефон на фронталке только потому, что шаг посреди плана провалился:
//!   после провала исполнитель делает те же шаги возврата (по возможности), а потом сообщает исходный провал.
//! * **Чужие события.** `LensSwitchFailed` или смена объектива МЕЖДУ шагами и после последнего — тоже провал: шаг,
//!   который их не ждал, их не судит.

use camera_core::CameraEvent;

use super::zoom_plan::{ZoomAction, ZoomStep};
use super::zoom_views::{EngineView, ScreenView};
use crate::camera_screen_source::ZoomView;
use camera_contract::ids::{LensFacing, LensRole};

/// Допуск «экран на 1×»: 2 % коэффициента в тысячных (как в вердикте).
const ONE_X_TOLERANCE_MILLI: u32 = 20;
/// 1× в тысячных.
const ONE_X_MILLI: u32 = 1_000;

/// Зачем шаг в очереди исполнителя.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepRole {
    /// Шаг плана: попадает в отчёт и судится вердиктом.
    Planned,
    /// Приведение к дому перед планом: в отчёт попадает, только если провалилось.
    Preparation,
    /// Возврат к дому после провала: в отчёт не попадает, результат виден по итоговому состоянию экрана.
    Restoration,
}

/// Шаг очереди с его ролью.
#[derive(Debug, Clone, PartialEq)]
pub struct QueuedStep {
    pub step: ZoomStep,
    pub role: StepRole,
}

impl QueuedStep {
    pub fn new(step: ZoomStep, role: StepRole) -> Self {
        Self { step, role }
    }
}

/// Стоит ли телефон «дома»: задняя основная камера, 1×, колесо закрыто, плавного перехода нет.
fn is_home(engine: &EngineView, zoom: ZoomView) -> bool {
    let on_main_back = engine
        .active
        .as_ref()
        .is_some_and(|active| active.facing == LensFacing::Back && active.role == LensRole::Wide);
    on_main_back
        && zoom.facing == LensFacing::Back
        && zoom.ratio_milli.abs_diff(ONE_X_MILLI) <= ONE_X_TOLERANCE_MILLI
        && !zoom.wheel_open
        && !zoom.gliding
}

/// Шаги, которые приводят экран к дому; пусто, если он уже дома. `prefix` — начало имени шагов.
pub fn steps_to_home(engine: &EngineView, screen: &ScreenView, prefix: &str) -> Vec<ZoomStep> {
    if is_home(engine, screen.zoom) {
        return Vec::new();
    }
    let mut steps = Vec::new();
    let on_front = engine
        .active
        .as_ref()
        .is_some_and(|active| active.facing == LensFacing::Front);
    if on_front {
        steps.push(ZoomStep {
            name: format!("{prefix}_flip_to_back"),
            action: ZoomAction::Flip,
        });
    }
    steps.push(ZoomStep {
        name: format!("{prefix}_tap_1"),
        action: ZoomAction::TapStop { ratio: 1.0 },
    });
    steps
}

/// Провал по событию движка, которое пришло, когда НИ ОДИН шаг его не ждал (между шагами или после последнего).
/// Остальные события (повторные цели, деградация частоты, статусы) к суду не относятся.
pub fn stray_event_failure(events: &[CameraEvent], context: &str) -> Option<String> {
    events.iter().find_map(|event| match event {
        CameraEvent::LensSwitchFailed { from, to, reason } => Some(format!(
            "{context}: смена объектива {}→{} не удалась, и её никто не судил: {reason:?}",
            from.name(),
            to.name()
        )),
        CameraEvent::LensSwitched { from, to, .. } => Some(format!(
            "{context}: движок сменил объектив {}→{}, а ни один шаг этого не просил",
            from.name(),
            to.name()
        )),
        _ => None,
    })
}

#[cfg(test)]
#[path = "zoom_home_tests.rs"]
mod tests;
