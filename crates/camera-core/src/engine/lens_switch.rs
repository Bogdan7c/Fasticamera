//! Машина состояний смены объектива (сессия 13) — чистая, без доступа к бэкенду и каталогу.
//!
//! Владеет четырьмя вещами: где мы в смене ([`LensSwitchState`]), когда смена началась,
//! блокировкой роли после провала и счётчиками. Входы — три факта (цель камеры поменялась,
//! пришёл первый кадр камеры роли X, новая камера отказала), выходы — события и решения.
//! Перестройкой потока по-прежнему владеет движок (`reconfiguring` в `reconcile`): здесь второго
//! владельца «перестройки» нет, только учёт того, что видит экран.
//!
//! Правила:
//! * смена начинается, когда цель меняется, пока уже показан объектив (`Steady`);
//!   первый показ (`Idle`) сменой не считается;
//! * «показан объектив» наступает в момент **старта потока** камеры ([`LensSwitchTracker::on_stream_started`],
//!   движок перешёл в `Streaming`), а не с первым кадром: экран считает камеру готовой по
//!   `Streaming`, и тап по зуму сразу после этого — уже смена с объектива, который показывают.
//!   Привязка к первому кадру теряла такую смену (находка на телефоне, 13a: ультраширик, нажатый до
//!   первого результата захвата, ушёл `Idle → Steady` без `LensSwitched`). Цель, изменившаяся
//!   **до** старта потока (камера открывается/настраивается, отложенное намерение), остаётся первым
//!   показом. Старт потока посреди смены её не завершает — только первый кадр новой камеры;
//! * завершается она **первым кадром новой камеры**, а не состоянием `Streaming`: экран держит
//!   размытый последний кадр, пока реальный кадр не пришёл (решение 110);
//! * цель поменялась посреди смены → прежний `from` остаётся, промежуточная цель отменяется
//!   явным событием; возврат на `from` — тоже смена (завершается кадром `from`);
//! * провал новой камеры возвращает цель на `from`, а роль `to` блокируется для селектора.

use std::time::Instant;

use camera_contract::availability::CameraUnavailableReason;
use camera_contract::ids::LensRole;

use crate::event::CameraEvent;
use crate::lens_status::{LensBlock, LensCounters, LensSwitchFailure, LensSwitchMethod, LensSwitchState};

/// Чья это цель: от неё зависит, снимается ли блокировка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RetargetCause {
    /// Команда `Open`/`SwitchTo`: явная просьба, снимает блокировку этой роли.
    Command,
    /// Намерение экрана: блокировку снимает селектор (см. `lens_intent`), а не сама цель.
    Intent,
}

/// Решение о возврате после провала новой камеры.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Revert {
    pub(super) from: LensRole,
    pub(super) to: LensRole,
}

/// Провал настолько зависит от самой новой камеры, что оправдывает возврат?
///
/// Вытеснение, «занята», лимит камер, разрешение и политика — свойства системы, а не камеры:
/// их лечит обычное восстановление (решение 39), возвращаться на старую камеру незачем.
pub(super) fn is_fault_of_new_camera(reason: CameraUnavailableReason) -> bool {
    matches!(
        reason,
        CameraUnavailableReason::DeviceFailure
            | CameraUnavailableReason::ServiceFailure
            | CameraUnavailableReason::CameraNotPresent
    )
}

/// Учёт смены объектива.
#[derive(Debug)]
pub(super) struct LensSwitchTracker {
    state: LensSwitchState,
    /// Когда началась текущая смена (первое намерение или команда); не сдвигается при
    /// пересмотре цели: экрану важно полное время от первого действия пользователя.
    started_at: Option<Instant>,
    blocked: Option<LensBlock>,
    counters: LensCounters,
    /// Как исполняется текущая смена; по умолчанию — переоткрытием. Повышение тёплой выставляет
    /// [`Self::note_promoted_standby`], любая новая цель сбрасывает обратно.
    method: LensSwitchMethod,
}

impl LensSwitchTracker {
    pub(super) fn new() -> Self {
        Self {
            state: LensSwitchState::Idle,
            started_at: None,
            blocked: None,
            counters: LensCounters::default(),
            method: LensSwitchMethod::default(),
        }
    }

    /// Текущая смена исполнена повышением тёплой камеры (обменом ролей), а не переоткрытием.
    pub(super) fn note_promoted_standby(&mut self) {
        self.method = LensSwitchMethod::PromotedStandby;
    }

    pub(super) fn state(&self) -> LensSwitchState {
        self.state
    }

    pub(super) fn blocked(&self) -> Option<&LensBlock> {
        self.blocked.as_ref()
    }

    pub(super) fn counters(&self) -> LensCounters {
        self.counters
    }

    pub(super) fn note_intent_ignored(&mut self) {
        self.counters.intents_ignored += 1;
    }

    /// Цель камеры поменялась на роль `new_role`. Возвращает событие отмены промежуточной цели,
    /// если смена уже шла.
    pub(super) fn on_target_changed(
        &mut self,
        new_role: LensRole,
        cause: RetargetCause,
        now: Instant,
    ) -> Option<CameraEvent> {
        // Способ исполнения определяется заново для каждой цели: прежнее повышение к ней не относится.
        self.method = LensSwitchMethod::default();
        if cause == RetargetCause::Command && self.blocked.as_ref().is_some_and(|block| block.role == new_role) {
            self.blocked = None;
        }
        match self.state {
            LensSwitchState::Idle => None,
            LensSwitchState::Steady { role } if role == new_role => None,
            LensSwitchState::Steady { role } => {
                self.state = LensSwitchState::Switching {
                    from: role,
                    to: new_role,
                };
                self.started_at = Some(now);
                None
            }
            LensSwitchState::Switching { to, .. } if to == new_role => None,
            LensSwitchState::Switching { from, to: abandoned } => {
                self.state = LensSwitchState::Switching { from, to: new_role };
                self.counters.switches_retargeted += 1;
                Some(CameraEvent::LensSwitchRetargeted {
                    from,
                    abandoned,
                    to: new_role,
                })
            }
        }
    }

    /// Поток камеры роли `role` запущен (движок перешёл в `Streaming`).
    ///
    /// * `Idle` → `Steady { role }`: первый показ закреплён за стартом потока, а не за первым кадром,
    ///   поэтому смена цели сразу после `Streaming` уже считается сменой объектива;
    /// * `Switching` — без изменений: смену завершает только первый кадр новой камеры (решение 110);
    /// * `Steady` — без изменений. `Steady { X }` и старт потока другой роли без смены цели не
    ///   возникают (любая смена цели идёт через [`Self::on_target_changed`] и даёт `Switching`);
    ///   если всё же случится, решает кадр ([`Self::on_frame`]), как и раньше.
    ///
    /// Событий и счётчиков не создаёт: первый показ — не смена.
    pub(super) fn on_stream_started(&mut self, role: LensRole) {
        if self.state == LensSwitchState::Idle {
            self.state = LensSwitchState::Steady { role };
        }
    }

    /// Цель стала камерой без продуктовой роли (диагностика): показывать объективы не о чем.
    pub(super) fn on_target_without_role(&mut self) {
        self.reset();
    }

    /// Пришёл кадр повторяющегося запроса камеры роли `role`.
    pub(super) fn on_frame(&mut self, role: LensRole, now: Instant) -> Option<CameraEvent> {
        match self.state {
            LensSwitchState::Idle | LensSwitchState::Steady { .. } => {
                self.state = LensSwitchState::Steady { role };
                None
            }
            LensSwitchState::Switching { from, to } if to == role => {
                self.state = LensSwitchState::Steady { role };
                let elapsed = self.started_at.take().map_or(std::time::Duration::ZERO, |started| {
                    now.saturating_duration_since(started)
                });
                if from != to {
                    self.counters.switches_completed += 1;
                }
                self.counters.last_switch_elapsed = Some(elapsed);
                Some(CameraEvent::LensSwitched {
                    from,
                    to,
                    elapsed,
                    method: std::mem::take(&mut self.method),
                })
            }
            // Кадр не той камеры, которую ждём (запоздавший): смену он не завершает.
            LensSwitchState::Switching { .. } => None,
        }
    }

    /// Нужно ли вернуться назад, если новая камера отказала? Только пока идёт смена на другой
    /// объектив: возврат на `from` уже пробуем (`from == to`) — дальше обычное восстановление.
    pub(super) fn revert_candidate(&self) -> Option<Revert> {
        match self.state {
            LensSwitchState::Switching { from, to } if from != to => Some(Revert { from, to }),
            _ => None,
        }
    }

    /// Новая камера отказала: вернуть цель на `from`, заблокировать `to`.
    pub(super) fn on_switch_failed(&mut self, revert: Revert, reason: LensSwitchFailure) -> CameraEvent {
        self.method = LensSwitchMethod::default();
        self.state = LensSwitchState::Switching {
            from: revert.from,
            to: revert.from,
        };
        self.blocked = Some(LensBlock {
            role: revert.to,
            reason: reason.clone(),
        });
        self.counters.switches_failed += 1;
        CameraEvent::LensSwitchFailed {
            from: revert.from,
            to: revert.to,
            reason,
        }
    }

    /// Снять блокировку: намерение выбрало другую роль.
    pub(super) fn clear_block(&mut self) {
        self.blocked = None;
    }

    /// Показывать нечего (камера закрыта, поток остановлен, окончательная ошибка).
    pub(super) fn reset(&mut self) {
        self.state = LensSwitchState::Idle;
        self.started_at = None;
        self.blocked = None;
        self.method = LensSwitchMethod::default();
    }
}

#[cfg(test)]
#[path = "lens_switch_tests.rs"]
mod tests;
