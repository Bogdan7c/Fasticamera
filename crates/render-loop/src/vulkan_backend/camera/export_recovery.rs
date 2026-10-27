//! Восстановление экспорта release-fence без ожиданий GPU на потоке рендера.
//!
//! Экспорт fence мог не удаться: бинарный семафор остаётся «ждущим экспорта» (его payload неизвестен) и
//! повторно сигналить его нельзя. Пока он «залип», новые экспорты не просим: уходящие лизинги возвращаются
//! по завершении своей серии сабмитов (`LeaseLedger::release_retiring_completed`). Заменить семафор
//! можно, когда GPU закончил сабмит, который его сигналил (`completed >= signaled_by`) — это известно
//! без ожидания (fence слота ждёт acquire кадра). Создание нового семафора может отказывать —
//! повторяем с растущей паузой в КАДРАХ (не во времени и не ожиданием GPU) и сдаёмся после
//! [`MAX_RECREATE_ATTEMPTS`] неудач.

use gpu_vulkan::hardware_buffer_cache::SubmissionSerial;

/// Сколько неудачных попыток создать семафор допускаем, прежде чем отказаться от экспорта до конца сессии.
/// Отказ не опасен: лизинги и так возвращаются по завершению серий, просто камера получает буферы чуть
/// позже (на 1-2 кадра), чем по fence.
pub const MAX_RECREATE_ATTEMPTS: u32 = 8;
/// Пауза после первой неудачи, кадров (~70 мс при 60 к/с); дальше удваивается.
pub const FIRST_RETRY_DELAY_FRAMES: u32 = 4;
/// Потолок паузы между попытками, кадров (~4 с при 60 к/с): редкие попытки не нагружают поток рендера,
/// а восстановление при временной нехватке ресурсов остаётся быстрым.
pub const MAX_RETRY_DELAY_FRAMES: u32 = 240;

/// Что делать на этом кадре с семафором экспорта.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryStep {
    /// Ничего.
    Nothing,
    /// Попробовать создать новый семафор и сообщить итог ([`ExportRecovery::note_recreate_result`]).
    TryRecreate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Usable,
    Stuck {
        /// Серия сабмита, сигналившего залипший семафор.
        signaled_by: SubmissionSerial,
        failures: u32,
        frames_until_retry: u32,
    },
    GivenUp,
}

/// Состояние экспорта release-fence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportRecovery {
    phase: Phase,
    recreate_failures: u64,
}

impl ExportRecovery {
    /// Экспорт работает.
    pub const fn new() -> Self {
        Self {
            phase: Phase::Usable,
            recreate_failures: 0,
        }
    }

    /// Можно ли просить экспорт в следующем сабмите.
    pub fn can_export(&self) -> bool {
        self.phase == Phase::Usable
    }

    /// Сколько раз не удалось создать новый семафор за всё время.
    pub fn recreate_failures(&self) -> u64 {
        self.recreate_failures
    }

    /// Экспорт после сабмита серии `signaled_by` не удался.
    pub fn note_export_failed(&mut self, signaled_by: SubmissionSerial) {
        if self.phase == Phase::Usable {
            self.phase = Phase::Stuck {
                signaled_by,
                failures: 0,
                frames_until_retry: 0,
            };
        }
    }

    /// Вызывается один раз за кадр. `completed` — серия, до которой всё завершено.
    pub fn on_frame(&mut self, completed: SubmissionSerial) -> RecoveryStep {
        let Phase::Stuck {
            signaled_by,
            failures,
            frames_until_retry,
        } = self.phase
        else {
            return RecoveryStep::Nothing;
        };
        if completed.0 < signaled_by.0 {
            // GPU ещё может исполнять сигнал залипшего семафора: заменять рано.
            return RecoveryStep::Nothing;
        }
        if frames_until_retry > 0 {
            self.phase = Phase::Stuck {
                signaled_by,
                failures,
                frames_until_retry: frames_until_retry - 1,
            };
            return RecoveryStep::Nothing;
        }
        RecoveryStep::TryRecreate
    }

    /// Итог попытки создать семафор.
    pub fn note_recreate_result(&mut self, created: bool) {
        let Phase::Stuck {
            signaled_by, failures, ..
        } = self.phase
        else {
            return;
        };
        if created {
            self.phase = Phase::Usable;
            return;
        }
        self.recreate_failures += 1;
        let failures = failures + 1;
        self.phase = if failures >= MAX_RECREATE_ATTEMPTS {
            Phase::GivenUp
        } else {
            let delay = (FIRST_RETRY_DELAY_FRAMES << (failures - 1).min(16)).min(MAX_RETRY_DELAY_FRAMES);
            Phase::Stuck {
                signaled_by,
                failures,
                frames_until_retry: delay,
            }
        };
    }
}

impl Default for ExportRecovery {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIGNALED_BY: SubmissionSerial = SubmissionSerial(10);

    fn stuck() -> ExportRecovery {
        let mut recovery = ExportRecovery::new();
        assert!(recovery.can_export());
        recovery.note_export_failed(SIGNALED_BY);
        assert!(!recovery.can_export(), "залипший семафор не просим экспортировать");
        recovery
    }

    #[test]
    fn semaphore_is_not_replaced_while_gpu_may_still_run_its_signal() {
        let mut recovery = stuck();
        assert_eq!(recovery.on_frame(SubmissionSerial(9)), RecoveryStep::Nothing);
        assert_eq!(recovery.on_frame(SIGNALED_BY), RecoveryStep::TryRecreate);
        recovery.note_recreate_result(true);
        assert!(recovery.can_export());
        assert_eq!(recovery.recreate_failures(), 0);
    }

    #[test]
    fn failed_recreation_backs_off_in_frames_counts_failures_and_eventually_gives_up() {
        let mut recovery = stuck();
        let mut attempts = 0;
        let mut frames_between = Vec::new();
        let mut since_last = 0;
        for _ in 0..100_000 {
            match recovery.on_frame(SubmissionSerial(100)) {
                RecoveryStep::TryRecreate => {
                    attempts += 1;
                    frames_between.push(since_last);
                    since_last = 0;
                    recovery.note_recreate_result(false);
                }
                RecoveryStep::Nothing => since_last += 1,
            }
        }
        assert_eq!(attempts, MAX_RECREATE_ATTEMPTS, "попытки ограничены");
        assert_eq!(recovery.recreate_failures(), u64::from(MAX_RECREATE_ATTEMPTS));
        assert!(!recovery.can_export(), "сдались: экспорта больше нет");
        assert_eq!(frames_between[1], FIRST_RETRY_DELAY_FRAMES, "первая пауза");
        assert_eq!(frames_between[2], FIRST_RETRY_DELAY_FRAMES * 2, "пауза удваивается");
        assert!(frames_between.iter().all(|gap| *gap <= MAX_RETRY_DELAY_FRAMES));
    }
}
