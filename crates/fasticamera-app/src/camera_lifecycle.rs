//! Политика жизненного цикла камеры в приложении: что делать с камерой, когда приложение уходит в фон.
//!
//! Android не даёт камеру фоновому приложению, а Oplus замораживает фоновые процессы, поэтому по
//! умолчанию камера в фоне **отпускается** (`ReleaseInBackground`): движок получает `enter_background()`,
//! закрывает устройство и сохраняет желаемое состояние, а при возврате восстанавливает его.
//!
//! Исключение одно: отладочный сценарий (`camera-evict`) должен пережить уход в фон с открытой
//! камерой, чтобы увидеть вытеснение системной камерой. Он временно включает
//! `HoldInBackgroundForDiagnostics`. Ответственность за возврат политики лежит **не на сценарии**, а
//! на маршрутизаторе: он вызывает [`CameraLifecycleState::restore_default`] при любом конце сценария
//! (успех, провал, вытеснение другим сценарием, паника, дедлайн), потому что любой конец проходит
//! через его `finish`.
//!
//! Модуль владеет только состоянием политики и «заявкой» сценария на камеру. Команд движку он не
//! отправляет — это делает `CameraServices`.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use serde::Serialize;

/// Что делать с камерой, когда приложение ушло в фон.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraLifecyclePolicy {
    /// По умолчанию: фон → `enter_background()`, камера отпущена.
    ReleaseInBackground,
    /// Только для активного отладочного сценария: фон не отпускает камеру (она может быть вытеснена,
    /// и движок должен это пережить).
    HoldInBackgroundForDiagnostics,
}

impl CameraLifecyclePolicy {
    const fn to_raw(self) -> u8 {
        match self {
            CameraLifecyclePolicy::ReleaseInBackground => 0,
            CameraLifecyclePolicy::HoldInBackgroundForDiagnostics => 1,
        }
    }

    /// Неизвестное число — безопасная политика по умолчанию: лучше отпустить камеру, чем держать.
    const fn from_raw(raw: u8) -> Self {
        match raw {
            1 => CameraLifecyclePolicy::HoldInBackgroundForDiagnostics,
            _ => CameraLifecyclePolicy::ReleaseInBackground,
        }
    }
}

/// Что произошло при возврате политики по умолчанию.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyRestore {
    /// Политика и так была по умолчанию — ничего не менялось.
    AlreadyDefault,
    /// Была `HoldInBackgroundForDiagnostics`, возвращена по умолчанию: если приложение сейчас в
    /// фоне, камеру теперь надо отпустить (это решает вызывающий).
    RestoredFromHold,
}

/// Состояние политики и заявки сценария. Читается и меняется без замков: им пользуются
/// маршрутизатор (поток событий) и сценарии (его же поток), но тесты читают из других потоков.
#[derive(Debug, Default)]
pub struct CameraLifecycleState {
    policy: AtomicU8,
    /// Сценарий открыл камеру для диагностики: по его концу камеру нужно закрыть (желаемое забыть).
    scenario_claim: AtomicBool,
}

impl CameraLifecycleState {
    pub fn policy(&self) -> CameraLifecyclePolicy {
        CameraLifecyclePolicy::from_raw(self.policy.load(Ordering::SeqCst))
    }

    /// Сценарий просит держать камеру в фоне. Вместе с этим он заявляет права на камеру.
    pub fn hold_in_background_for_diagnostics(&self) {
        self.policy.store(
            CameraLifecyclePolicy::HoldInBackgroundForDiagnostics.to_raw(),
            Ordering::SeqCst,
        );
        self.scenario_claim.store(true, Ordering::SeqCst);
    }

    /// Сценарий начал пользоваться камерой: по его концу маршрутизатор её закроет.
    pub fn claim_for_scenario(&self) {
        self.scenario_claim.store(true, Ordering::SeqCst);
    }

    /// Возвращает политику по умолчанию. Идемпотентна: второй вызов — `AlreadyDefault`.
    pub fn restore_default(&self) -> PolicyRestore {
        let previous = self
            .policy
            .swap(CameraLifecyclePolicy::ReleaseInBackground.to_raw(), Ordering::SeqCst);
        match CameraLifecyclePolicy::from_raw(previous) {
            CameraLifecyclePolicy::HoldInBackgroundForDiagnostics => PolicyRestore::RestoredFromHold,
            CameraLifecyclePolicy::ReleaseInBackground => PolicyRestore::AlreadyDefault,
        }
    }

    /// Есть ли заявка сценария на камеру прямо сейчас (не снимая её).
    pub fn has_scenario_claim(&self) -> bool {
        self.scenario_claim.load(Ordering::SeqCst)
    }

    /// Снимает заявку сценария; `true`, если заявка была (камеру надо закрыть).
    pub fn take_scenario_claim(&self) -> bool {
        self.scenario_claim.swap(false, Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_releases_in_background_and_has_no_claim() {
        let state = CameraLifecycleState::default();
        assert_eq!(state.policy(), CameraLifecyclePolicy::ReleaseInBackground);
        assert!(!state.take_scenario_claim());
    }

    #[test]
    fn hold_changes_policy_claims_the_camera_and_restore_is_idempotent() {
        let state = CameraLifecycleState::default();
        state.hold_in_background_for_diagnostics();
        assert_eq!(state.policy(), CameraLifecyclePolicy::HoldInBackgroundForDiagnostics);
        assert_eq!(state.restore_default(), PolicyRestore::RestoredFromHold);
        assert_eq!(state.policy(), CameraLifecyclePolicy::ReleaseInBackground);
        assert_eq!(state.restore_default(), PolicyRestore::AlreadyDefault);
        assert!(state.take_scenario_claim(), "заявка от hold остаётся до явного снятия");
        assert!(!state.take_scenario_claim(), "заявка снимается один раз");
    }

    #[test]
    fn claim_alone_does_not_change_the_policy() {
        let state = CameraLifecycleState::default();
        state.claim_for_scenario();
        assert_eq!(state.policy(), CameraLifecyclePolicy::ReleaseInBackground);
        assert_eq!(state.restore_default(), PolicyRestore::AlreadyDefault);
        assert!(state.take_scenario_claim());
    }

    #[test]
    fn unknown_raw_policy_falls_back_to_the_safe_default() {
        assert_eq!(
            CameraLifecyclePolicy::from_raw(200),
            CameraLifecyclePolicy::ReleaseInBackground
        );
    }
}
