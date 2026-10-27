//! Подделка службы подсказок производительности для тестов других crate-ов (фича
//! `fake-performance-hints`): записывает каждое действие в общий журнал и умеет отказывать по
//! команде теста. Журнал — `Arc<Mutex<…>>`, чтобы тест читал его, пока поток-потребитель работает.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use super::{FrameWork, FrameWorkSession, HintThreadId, PerformanceHintError, PerformanceHintService};

/// Что видела подделка.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FakeHintJournal {
    /// Потоки каждой созданной сессии (по порядку создания).
    pub sessions_created: Vec<Vec<HintThreadId>>,
    /// Начальная цель каждой созданной сессии.
    pub initial_targets: Vec<Duration>,
    /// Сколько сессий закрыто (`Drop`).
    pub sessions_closed: u64,
    /// Принятые отчёты о кадрах (все сессии подряд).
    pub reports: Vec<FrameWork>,
    /// Принятые новые цели.
    pub target_updates: Vec<Duration>,
    /// Сколько попыток создать сессию было (включая отказы).
    pub create_attempts: u64,
    /// Имя потока, с которого создана каждая сессия (проверка владения: сессию создаёт сам поток-потребитель).
    pub creating_threads: Vec<Option<String>>,
}

/// Чем отвечать на вызовы (меняется тестом на ходу).
#[derive(Debug, Clone, Default)]
pub struct FakeHintBehavior {
    /// Ошибка создания сессии (`None` — создаётся).
    pub create_error: Option<PerformanceHintError>,
    /// Ошибка отчётов о кадрах (`None` — принимаются).
    pub report_error: Option<PerformanceHintError>,
}

#[derive(Debug, Default)]
struct Shared {
    journal: FakeHintJournal,
    behavior: FakeHintBehavior,
}

/// Подделка службы. Клон смотрит в тот же журнал.
#[derive(Debug, Clone, Default)]
pub struct FakePerformanceHints {
    shared: Arc<Mutex<Shared>>,
    /// Какой «tid» отдаёт `current_thread` (тесту важно, что именно он попал в сессию).
    thread_id: i32,
}

impl FakePerformanceHints {
    /// Подделка, которая отдаёт потребителю «поток» `thread_id`.
    pub fn new(thread_id: i32) -> Self {
        Self {
            shared: Arc::default(),
            thread_id,
        }
    }

    /// Снимок журнала.
    pub fn journal(&self) -> FakeHintJournal {
        self.lock().journal.clone()
    }

    /// Меняет ответы подделки.
    pub fn set_behavior(&self, behavior: FakeHintBehavior) {
        self.lock().behavior = behavior;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl PerformanceHintService for FakePerformanceHints {
    fn current_thread(&self) -> Result<HintThreadId, PerformanceHintError> {
        Ok(HintThreadId::from_raw(self.thread_id))
    }

    fn create_session(
        &self,
        threads: &[HintThreadId],
        target: Duration,
    ) -> Result<Box<dyn FrameWorkSession>, PerformanceHintError> {
        let mut shared = self.lock();
        shared.journal.create_attempts += 1;
        if let Some(error) = shared.behavior.create_error.clone() {
            return Err(error);
        }
        shared.journal.sessions_created.push(threads.to_vec());
        shared
            .journal
            .creating_threads
            .push(std::thread::current().name().map(str::to_owned));
        shared.journal.initial_targets.push(target);
        Ok(Box::new(FakeSession {
            shared: Arc::clone(&self.shared),
        }))
    }
}

/// Сессия подделки.
struct FakeSession {
    shared: Arc<Mutex<Shared>>,
}

impl FakeSession {
    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl FrameWorkSession for FakeSession {
    fn update_target(&mut self, target: Duration) -> Result<(), PerformanceHintError> {
        self.lock().journal.target_updates.push(target);
        Ok(())
    }

    fn report_frame(&mut self, work: &FrameWork) -> Result<(), PerformanceHintError> {
        let mut shared = self.lock();
        if let Some(error) = shared.behavior.report_error.clone() {
            return Err(error);
        }
        shared.journal.reports.push(*work);
        Ok(())
    }
}

impl Drop for FakeSession {
    fn drop(&mut self) {
        self.lock().journal.sessions_closed += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::MonotonicNanos;

    #[test]
    fn fake_records_lifecycle_and_obeys_behavior() {
        let fake = FakePerformanceHints::new(42);
        let tid = fake.current_thread().expect("tid");
        let mut session = fake.create_session(&[tid], Duration::from_millis(12)).expect("сессия");
        let work = FrameWork::new(
            MonotonicNanos::from_nanos(5),
            Duration::from_millis(3),
            Duration::from_millis(1),
            None,
        )
        .expect("работа");
        session.report_frame(&work).expect("отчёт");
        fake.set_behavior(FakeHintBehavior {
            report_error: Some(PerformanceHintError::Failed {
                operation: "report",
                code: 32,
            }),
            ..FakeHintBehavior::default()
        });
        assert!(session.report_frame(&work).is_err());
        drop(session);
        let journal = fake.journal();
        assert_eq!(journal.sessions_created, vec![vec![HintThreadId::from_raw(42)]]);
        assert_eq!(journal.initial_targets, vec![Duration::from_millis(12)]);
        assert_eq!(journal.reports, vec![work], "отказанный отчёт не записан");
        assert_eq!(journal.sessions_closed, 1);
    }
}
