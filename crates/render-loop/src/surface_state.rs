//! Чистая машина состояний surface (без GPU и без потоков).
//!
//! Машина получает события жизненного цикла и отвечает СПИСКОМ действий, которые поток
//! рендера обязан выполнить именно в этом порядке. Сама она ничего не держит: ни
//! `NativeSurface`, ни квитанции, ни бэкенд — только номера поколений. Благодаря этому все
//! сложные случаи (две surface подряд, устаревшие события, отказ attach) проверяются
//! обычными юнит-тестами.
//!
//! Главный инвариант: действие [`SurfaceAction::ConfirmRelease`] всегда стоит в списке ПОСЛЕ
//! [`SurfaceAction::Detach`] / [`SurfaceAction::DiscardUnattached`] той же surface:
//! квитанция подтверждается, только когда ничего не использует окно.
//!
//! Выбор по паузе: пока показ на паузе, surface остаётся присоединённой (swapchain жив,
//! кадры не рисуются). Пересоздавать цепочку при каждой паузе дороже и бессмысленно; если
//! Android заберёт окно, придёт `destroyed`, и обычный путь освобождения сработает.

use shell_contract::surface::{SurfaceGeneration, SurfaceSize};

/// Почему текущая surface непригодна, хотя Android её ещё не забрал.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnusableCause {
    /// Бэкенд отказался присоединить surface (`attach` вернул ошибку).
    AttachFailed,
    /// Во время показа бэкенд сообщил, что surface потеряна.
    SurfaceLost,
}

/// Текущее состояние (для статуса и тестов).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfacePhase {
    /// Surface нет: поток спит.
    NoSurface,
    /// Surface создана, размера ещё нет: присоединить нельзя.
    AwaitingSize { generation: SurfaceGeneration },
    /// Surface присоединена к бэкенду.
    Attached {
        generation: SurfaceGeneration,
        size: SurfaceSize,
    },
    /// Бэкенд surface не держит, но Android ещё не забрал её: ждём `destroyed`.
    Unusable {
        generation: SurfaceGeneration,
        cause: UnusableCause,
    },
    /// Поток вышел из строя (паника бэкенда и т. п.): новые surface не принимаются,
    /// запросы освобождения подтверждаются сразу.
    Faulted,
}

/// Что должен сделать поток рендера.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceAction {
    /// Присоединить хранимую surface этого поколения к бэкенду с данным размером.
    Attach {
        generation: SurfaceGeneration,
        size: SurfaceSize,
    },
    /// Сообщить бэкенду новый размер (пересоздание swapchain).
    Resize {
        generation: SurfaceGeneration,
        size: SurfaceSize,
    },
    /// Отсоединить surface от бэкенда (GPU дожидается, swapchain уничтожается).
    Detach { generation: SurfaceGeneration },
    /// Выбросить хранимую, но так и не присоединённую surface.
    DiscardUnattached { generation: SurfaceGeneration },
    /// Отвергнуть входящую surface (поток неисправен или поколение устарело): просто уничтожить.
    RejectIncoming { generation: SurfaceGeneration },
    /// Подтвердить квитанцию освобождения этого поколения. Идёт только после отпускания.
    ConfirmRelease { generation: SurfaceGeneration },
}

/// Сама машина.
#[derive(Debug)]
pub struct SurfaceStateMachine {
    phase: SurfacePhase,
    paused: bool,
    /// Самое большое поколение, о котором мы слышали: меньшее по номеру `created` — устаревшее.
    newest_generation: Option<SurfaceGeneration>,
    stale_changes: u64,
}

impl Default for SurfaceStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl SurfaceStateMachine {
    pub fn new() -> Self {
        Self {
            phase: SurfacePhase::NoSurface,
            paused: false,
            newest_generation: None,
            stale_changes: 0,
        }
    }

    pub fn phase(&self) -> SurfacePhase {
        self.phase
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Сколько раз `changed` пришёл для не-текущего поколения и был проигнорирован.
    pub fn stale_changes(&self) -> u64 {
        self.stale_changes
    }

    /// Нужно ли рисовать кадры сейчас: surface присоединена и показ не на паузе.
    pub fn should_present(&self) -> bool {
        matches!(self.phase, SurfacePhase::Attached { .. }) && !self.paused
    }

    /// Поколение surface, которой владеет машина (любая фаза, кроме `NoSurface`/`Faulted`).
    pub fn current_generation(&self) -> Option<SurfaceGeneration> {
        match self.phase {
            SurfacePhase::AwaitingSize { generation }
            | SurfacePhase::Attached { generation, .. }
            | SurfacePhase::Unusable { generation, .. } => Some(generation),
            SurfacePhase::NoSurface | SurfacePhase::Faulted => None,
        }
    }

    /// Появилась новая surface. Если старая ещё не освобождена, она отпускается ПЕРЕД
    /// новой: бэкенд держит одну цель, а два окна одновременно — гонка за swapchain.
    pub fn on_created(&mut self, generation: SurfaceGeneration) -> Vec<SurfaceAction> {
        let is_stale = self.newest_generation.is_some_and(|newest| generation <= newest);
        if self.phase == SurfacePhase::Faulted || is_stale {
            return vec![SurfaceAction::RejectIncoming { generation }];
        }
        let actions = self.release_current_actions();
        self.newest_generation = Some(generation);
        self.phase = SurfacePhase::AwaitingSize { generation };
        actions
    }

    /// Пришёл размер surface.
    pub fn on_changed(&mut self, generation: SurfaceGeneration, size: SurfaceSize) -> Vec<SurfaceAction> {
        if self.current_generation() != Some(generation) {
            self.stale_changes += 1;
            return Vec::new();
        }
        match self.phase {
            SurfacePhase::AwaitingSize { .. } => vec![SurfaceAction::Attach { generation, size }],
            SurfacePhase::Attached { size: current, .. } if current != size => {
                self.phase = SurfacePhase::Attached { generation, size };
                vec![SurfaceAction::Resize { generation, size }]
            }
            // Тот же размер (повторное уведомление) или surface уже непригодна: делать нечего.
            _ => Vec::new(),
        }
    }

    /// Поток выполнил [`SurfaceAction::Attach`] успешно.
    pub fn on_attach_succeeded(&mut self, generation: SurfaceGeneration, size: SurfaceSize) {
        if self.phase == (SurfacePhase::AwaitingSize { generation }) {
            self.phase = SurfacePhase::Attached { generation, size };
        }
    }

    /// Поток выполнил [`SurfaceAction::Attach`] с ошибкой: бэкенд surface не держит.
    pub fn on_attach_failed(&mut self, generation: SurfaceGeneration) {
        if self.phase == (SurfacePhase::AwaitingSize { generation }) {
            self.phase = SurfacePhase::Unusable {
                generation,
                cause: UnusableCause::AttachFailed,
            };
        }
    }

    /// Бэкенд сообщил `SurfaceLost`: отпускаем его цель, но квитанцию ждём от Android.
    pub fn on_surface_lost(&mut self) -> Vec<SurfaceAction> {
        match self.phase {
            SurfacePhase::Attached { generation, .. } => {
                self.phase = SurfacePhase::Unusable {
                    generation,
                    cause: UnusableCause::SurfaceLost,
                };
                vec![SurfaceAction::Detach { generation }]
            }
            _ => Vec::new(),
        }
    }

    /// Android забирает surface данного поколения.
    pub fn on_destroyed(&mut self, generation: SurfaceGeneration) -> Vec<SurfaceAction> {
        if self.current_generation() != Some(generation) {
            // Старая (уже отпущенная при повторном `created`) или неизвестная: окно
            // у нас не занято, подтверждаем сразу.
            return vec![SurfaceAction::ConfirmRelease { generation }];
        }
        let mut actions = self.release_current_actions();
        actions.push(SurfaceAction::ConfirmRelease { generation });
        actions
    }

    /// Поток завершается: отпускает текущую surface (если есть) и возвращается в `NoSurface`.
    /// Квитанций тут нет — оставшиеся в ящике запросы обрабатываются обычным `on_destroyed`.
    pub fn on_shutdown(&mut self) -> Vec<SurfaceAction> {
        self.release_current_actions()
    }

    pub fn on_paused(&mut self) {
        self.paused = true;
    }

    pub fn on_resumed(&mut self) {
        self.paused = false;
    }

    /// Поток неисправен. Отпускать нечего (бэкенд уже не вызывается как исправный),
    /// дальше действует только подтверждение чужих запросов.
    pub fn on_fault(&mut self) {
        self.phase = SurfacePhase::Faulted;
    }

    /// Действия, отпускающие текущую surface, и переход в `NoSurface`.
    fn release_current_actions(&mut self) -> Vec<SurfaceAction> {
        let actions = match self.phase {
            SurfacePhase::Attached { generation, .. } => vec![SurfaceAction::Detach { generation }],
            SurfacePhase::AwaitingSize { generation } => {
                vec![SurfaceAction::DiscardUnattached { generation }]
            }
            // У бэкенда уже ничего нет (отказ attach / потеря surface).
            SurfacePhase::Unusable { .. } | SurfacePhase::NoSurface | SurfacePhase::Faulted => Vec::new(),
        };
        if self.phase != SurfacePhase::Faulted {
            self.phase = SurfacePhase::NoSurface;
        }
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: SurfaceSize = SurfaceSize {
        width_px: 1440,
        height_px: 3216,
    };

    fn generation(value: u64) -> SurfaceGeneration {
        SurfaceGeneration::new(value)
    }

    /// Машина с присоединённой surface поколения 1.
    fn attached_machine() -> SurfaceStateMachine {
        let mut machine = SurfaceStateMachine::new();
        machine.on_created(generation(1));
        machine.on_changed(generation(1), SIZE);
        machine.on_attach_succeeded(generation(1), SIZE);
        machine
    }

    #[test]
    fn created_then_changed_attaches_and_starts_presenting() {
        let mut machine = SurfaceStateMachine::new();
        assert!(machine.on_created(generation(1)).is_empty());
        assert!(!machine.should_present(), "без размера показывать нельзя");
        assert_eq!(
            machine.on_changed(generation(1), SIZE),
            [SurfaceAction::Attach {
                generation: generation(1),
                size: SIZE
            }]
        );
        machine.on_attach_succeeded(generation(1), SIZE);
        assert!(machine.should_present());
        assert_eq!(
            machine.phase(),
            SurfacePhase::Attached {
                generation: generation(1),
                size: SIZE
            }
        );
    }

    #[test]
    fn changed_with_new_size_resizes_and_same_size_is_noop() {
        let mut machine = attached_machine();
        assert!(machine.on_changed(generation(1), SIZE).is_empty());
        let bigger = SurfaceSize {
            width_px: 3216,
            height_px: 1440,
        };
        assert_eq!(
            machine.on_changed(generation(1), bigger),
            [SurfaceAction::Resize {
                generation: generation(1),
                size: bigger
            }]
        );
        assert_eq!(
            machine.phase(),
            SurfacePhase::Attached {
                generation: generation(1),
                size: bigger
            }
        );
    }

    #[test]
    fn created_twice_detaches_old_before_new_is_accepted() {
        let mut machine = attached_machine();
        let actions = machine.on_created(generation(2));
        assert_eq!(
            actions,
            [SurfaceAction::Detach {
                generation: generation(1)
            }]
        );
        assert_eq!(
            machine.phase(),
            SurfacePhase::AwaitingSize {
                generation: generation(2)
            }
        );
        // Старая квитанция приходит позже — подтверждается сразу, ничего не отсоединяется.
        assert_eq!(
            machine.on_destroyed(generation(1)),
            [SurfaceAction::ConfirmRelease {
                generation: generation(1)
            }]
        );
        assert_eq!(
            machine.phase(),
            SurfacePhase::AwaitingSize {
                generation: generation(2)
            },
            "чужая квитанция не трогает новую surface"
        );
    }

    #[test]
    fn created_twice_before_size_discards_unattached_old() {
        let mut machine = SurfaceStateMachine::new();
        machine.on_created(generation(1));
        assert_eq!(
            machine.on_created(generation(2)),
            [SurfaceAction::DiscardUnattached {
                generation: generation(1)
            }]
        );
    }

    #[test]
    fn destroyed_current_detaches_then_confirms_in_that_order() {
        let mut machine = attached_machine();
        let actions = machine.on_destroyed(generation(1));
        assert_eq!(
            actions,
            [
                SurfaceAction::Detach {
                    generation: generation(1)
                },
                SurfaceAction::ConfirmRelease {
                    generation: generation(1)
                }
            ]
        );
        assert_eq!(machine.phase(), SurfacePhase::NoSurface);
        assert!(!machine.should_present());
    }

    #[test]
    fn destroyed_before_size_discards_then_confirms() {
        let mut machine = SurfaceStateMachine::new();
        machine.on_created(generation(1));
        assert_eq!(
            machine.on_destroyed(generation(1)),
            [
                SurfaceAction::DiscardUnattached {
                    generation: generation(1)
                },
                SurfaceAction::ConfirmRelease {
                    generation: generation(1)
                }
            ]
        );
    }

    #[test]
    fn destroyed_unknown_generation_confirms_without_touching_current() {
        let mut machine = attached_machine();
        assert_eq!(
            machine.on_destroyed(generation(99)),
            [SurfaceAction::ConfirmRelease {
                generation: generation(99)
            }]
        );
        assert!(machine.should_present(), "текущая surface осталась на месте");
    }

    #[test]
    fn destroyed_with_no_surface_at_all_confirms() {
        let mut machine = SurfaceStateMachine::new();
        assert_eq!(
            machine.on_destroyed(generation(5)),
            [SurfaceAction::ConfirmRelease {
                generation: generation(5)
            }]
        );
    }

    #[test]
    fn stale_changed_is_ignored_and_counted() {
        let mut machine = attached_machine();
        machine.on_created(generation(2));
        assert!(machine.on_changed(generation(1), SIZE).is_empty());
        assert!(machine.on_changed(generation(7), SIZE).is_empty());
        assert_eq!(machine.stale_changes(), 2);
        assert_eq!(
            machine.phase(),
            SurfacePhase::AwaitingSize {
                generation: generation(2)
            }
        );
    }

    #[test]
    fn pause_stops_presenting_but_keeps_surface_attached() {
        let mut machine = attached_machine();
        machine.on_paused();
        assert!(!machine.should_present());
        assert!(matches!(machine.phase(), SurfacePhase::Attached { .. }));
        machine.on_resumed();
        assert!(machine.should_present());
    }

    #[test]
    fn destroyed_while_paused_still_detaches_and_confirms() {
        let mut machine = attached_machine();
        machine.on_paused();
        let actions = machine.on_destroyed(generation(1));
        assert_eq!(actions.len(), 2);
        assert_eq!(
            actions[1],
            SurfaceAction::ConfirmRelease {
                generation: generation(1)
            }
        );
        assert!(machine.is_paused(), "пауза не связана с жизнью surface");
    }

    #[test]
    fn destroyed_after_failed_attach_confirms_without_detach() {
        let mut machine = SurfaceStateMachine::new();
        machine.on_created(generation(1));
        machine.on_changed(generation(1), SIZE);
        machine.on_attach_failed(generation(1));
        assert!(!machine.should_present());
        assert_eq!(
            machine.on_destroyed(generation(1)),
            [SurfaceAction::ConfirmRelease {
                generation: generation(1)
            }]
        );
        assert_eq!(machine.phase(), SurfacePhase::NoSurface);
    }

    #[test]
    fn surface_lost_detaches_once_and_waits_for_destroy() {
        let mut machine = attached_machine();
        assert_eq!(
            machine.on_surface_lost(),
            [SurfaceAction::Detach {
                generation: generation(1)
            }]
        );
        assert!(
            machine.on_surface_lost().is_empty(),
            "повторная потеря не отсоединяет второй раз"
        );
        assert_eq!(
            machine.on_destroyed(generation(1)),
            [SurfaceAction::ConfirmRelease {
                generation: generation(1)
            }]
        );
    }

    #[test]
    fn created_after_unusable_does_not_detach() {
        let mut machine = SurfaceStateMachine::new();
        machine.on_created(generation(1));
        machine.on_attach_failed(generation(1));
        assert!(machine.on_created(generation(2)).is_empty());
    }

    #[test]
    fn shutdown_releases_attached_surface_without_confirmation() {
        let mut machine = attached_machine();
        assert_eq!(
            machine.on_shutdown(),
            [SurfaceAction::Detach {
                generation: generation(1)
            }]
        );
        assert_eq!(machine.phase(), SurfacePhase::NoSurface);
        assert!(machine.on_shutdown().is_empty());
    }

    #[test]
    fn outdated_created_is_rejected() {
        let mut machine = attached_machine();
        assert_eq!(
            machine.on_created(generation(1)),
            [SurfaceAction::RejectIncoming {
                generation: generation(1)
            }]
        );
        assert!(machine.should_present(), "текущая surface не пострадала");
    }

    #[test]
    fn faulted_rejects_new_surfaces_and_confirms_every_release() {
        let mut machine = attached_machine();
        machine.on_fault();
        assert_eq!(machine.phase(), SurfacePhase::Faulted);
        assert!(!machine.should_present());
        assert_eq!(
            machine.on_created(generation(2)),
            [SurfaceAction::RejectIncoming {
                generation: generation(2)
            }]
        );
        assert_eq!(
            machine.on_destroyed(generation(1)),
            [SurfaceAction::ConfirmRelease {
                generation: generation(1)
            }]
        );
        assert_eq!(machine.phase(), SurfacePhase::Faulted, "неисправность необратима");
    }
}
