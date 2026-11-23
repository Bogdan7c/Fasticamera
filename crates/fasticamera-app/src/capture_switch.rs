//! Переключатель сервиса снимка за одним `Arc`: исполнитель эффектов держит ЕГО, а не конкретный сервис.
//!
//! # Зачем
//! Экран камеры строится один раз, и его исполнитель эффектов получает `Arc<dyn CaptureService>` навсегда. Сценарию
//! `screen`, который жмёт затвор, нельзя оставлять настоящую запись в галерею, а пересобирать из-за этого экран
//! нельзя (он остаётся на месте, `KeepRunning`). Переключатель решает это: ЗА тем же `Arc` меняется тот, кто
//! отвечает на запросы.
//!
//! # Инварианты
//! * Замок короткий и НЕ держится во время вызова сервиса: `submit` зовётся с потока рендера.
//! * `poll_completions` обходит и основной сервис, и подставной, и только что снятый подставной («дозревающий»):
//!   фейк отвечает лишь на опросе, и снимок, поставленный до возврата настоящего, иначе навсегда остался бы
//!   в ожидании ответа.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use camera_screen::shutter::ShotRequest;
use look_contract::LookSettings;
use shell_contract::clock::MonotonicNanos;

use crate::camera_screen_capture::{CaptureService, SubmitOutcome};

/// Кто сейчас отвечает на запросы сверх основного сервиса.
#[derive(Default)]
struct Substitution {
    /// Подставной сервис: пока он есть, запросы идут ему, а не основному.
    active: Option<Arc<dyn CaptureService>>,
    /// Снятый подставной: больше запросов не получает, но ещё отвечает на опросе за уже принятые.
    draining: Option<Arc<dyn CaptureService>>,
}

/// Переключатель сервиса снимка.
pub struct SwitchableCaptureService {
    primary: Arc<dyn CaptureService>,
    substitution: Mutex<Substitution>,
}

impl SwitchableCaptureService {
    pub fn new(primary: Arc<dyn CaptureService>) -> Self {
        Self {
            primary,
            substitution: Mutex::default(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Substitution> {
        self.substitution.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Направляет новые запросы подставному сервису.
    pub fn substitute(&self, replacement: Arc<dyn CaptureService>) {
        self.lock().active = Some(replacement);
    }

    /// Возвращает новые запросы основному сервису. `true` — подстановка была.
    pub fn restore(&self) -> bool {
        let mut substitution = self.lock();
        let Some(retired) = substitution.active.take() else {
            return false;
        };
        substitution.draining = Some(retired);
        true
    }
}

impl CaptureService for SwitchableCaptureService {
    fn submit(&self, request: ShotRequest, look: LookSettings, now: MonotonicNanos) -> SubmitOutcome {
        // Клон под замком, вызов — без него.
        let target = self.lock().active.clone().unwrap_or_else(|| Arc::clone(&self.primary));
        target.submit(request, look, now)
    }

    fn poll_completions(&self, now: MonotonicNanos) {
        let (active, draining) = {
            let substitution = self.lock();
            (substitution.active.clone(), substitution.draining.clone())
        };
        self.primary.poll_completions(now);
        for service in [active, draining].into_iter().flatten() {
            service.poll_completions(now);
        }
    }
}

#[cfg(test)]
#[path = "capture_switch_tests.rs"]
mod tests;
