//! Фоновая работа сценария: долгий вызов (создание GPU-контекста, аллокация AHB, шейдер) нельзя
//! делать в потоке маршрутизатора, иначе он перестанет читать события оболочки и `surfaceDestroyed`
//! зависнет. Сценарий запускает работу в своём потоке и опрашивает результат из `on_tick`.

use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::thread;
use std::time::{Duration, Instant};

use crate::jni_boundary::run_guarded;

/// Как идёт фоновая работа.
#[derive(Debug, PartialEq, Eq)]
pub enum JobPoll<T> {
    /// Ещё считает.
    Pending,
    /// Закончила с результатом.
    Done(T),
    /// Работа не закончилась за отведённое время (см. [`BackgroundJob::spawn`]). Поток работы
    /// остаётся жить (убить поток нельзя) и допишет результат в никому не нужный канал;
    /// сценарий обязан закончиться вердиктом, а не ждать вечно.
    TimedOut { after: Duration },
    /// Результата не будет: поток паниковал или исчез. Внутри — объяснение.
    Lost(String),
}

/// Результат фоновой работы, которого сценарий ждёт тиками.
pub struct BackgroundJob<T> {
    receiver: Receiver<Result<T, String>>,
    /// Сколько работе разрешено идти от запуска.
    timeout: Duration,
    /// Момент, после которого неготовый результат считается `TimedOut`.
    deadline: Instant,
}

impl<T: Send + 'static> BackgroundJob<T> {
    /// Запускает `work` в потоке `name`. Паника работы превращается в `Lost`, а не в падение процесса.
    /// Если за `timeout` результата нет, `poll` отвечает `TimedOut`: зависшая GPU-работа
    /// (драйвер, fence, который никогда не сигналит) не должна оставлять сценарий без вердикта.
    pub fn spawn(name: &str, timeout: Duration, work: impl FnOnce() -> T + Send + 'static) -> Result<Self, String> {
        let (sender, receiver) = channel();
        thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                let outcome =
                    run_guarded(work).map_err(|panic| format!("паника в фоновой работе: {}", panic.log_text()));
                // Получатель мог исчезнуть (сценарий вытеснен) — тогда результат просто не нужен.
                // Получатель ушёл (сценарий вытеснен) — результат некому отдать; это не ошибка
                // работы, но молча терять его нельзя: пишем в лог.
                if sender.send(outcome).is_err() {
                    tracing::warn!(target: "fasticamera::scenario", "фоновая работа завершилась, но её результат уже никто не ждёт");
                }
            })
            .map_err(|error| format!("поток {name} не создан: {error}"))?;
        Ok(Self {
            receiver,
            timeout,
            deadline: Instant::now() + timeout,
        })
    }

    /// Неблокирующий опрос; результат отдаётся один раз. Готовый результат важнее срока:
    /// работа, закончившая ровно вовремя, не превращается в `TimedOut`.
    pub fn poll(&self) -> JobPoll<T> {
        match self.receiver.try_recv() {
            Ok(Ok(value)) => JobPoll::Done(value),
            Ok(Err(reason)) => JobPoll::Lost(reason),
            Err(TryRecvError::Empty) if Instant::now() >= self.deadline => JobPoll::TimedOut { after: self.timeout },
            Err(TryRecvError::Empty) => JobPoll::Pending,
            Err(TryRecvError::Disconnected) => JobPoll::Lost("поток фоновой работы исчез без результата".to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::panic_report::install_panic_hook;

    fn wait_for_end<T: Send + 'static>(job: &BackgroundJob<T>) -> JobPoll<T> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match job.poll() {
                JobPoll::Pending if Instant::now() < deadline => thread::sleep(Duration::from_millis(1)),
                other => return other,
            }
        }
    }

    #[test]
    fn result_arrives_without_blocking_the_poller() {
        let (release, wait_release) = channel::<()>();
        let job = BackgroundJob::spawn("test-job", Duration::from_secs(5), move || {
            wait_release.recv().expect("сигнал");
            42
        })
        .expect("поток");
        assert_eq!(job.poll(), JobPoll::Pending, "работа ещё держится сигналом");
        release.send(()).expect("отпускаем");
        assert_eq!(wait_for_end(&job), JobPoll::Done(42));
    }

    #[test]
    fn panic_becomes_lost_with_message() {
        install_panic_hook();
        let job = BackgroundJob::<u32>::spawn("test-job-panic", Duration::from_secs(5), || {
            panic!("намеренный сбой работы")
        })
        .expect("поток");
        match wait_for_end(&job) {
            JobPoll::Lost(reason) => assert!(reason.contains("намеренный сбой работы"), "{reason}"),
            other => panic!("ожидалось Lost, получено {other:?}"),
        }
    }

    #[test]
    fn hung_work_times_out_but_late_or_ready_result_still_wins() {
        let (release, wait_release) = channel::<()>();
        let job = BackgroundJob::spawn("test-job-hung", Duration::from_millis(30), move || {
            wait_release.recv().expect("сигнал");
            7
        })
        .expect("поток");
        assert_eq!(job.poll(), JobPoll::Pending, "срок ещё не вышел");
        thread::sleep(Duration::from_millis(60));
        assert_eq!(
            job.poll(),
            JobPoll::TimedOut {
                after: Duration::from_millis(30)
            },
            "работа держится, срок вышел"
        );
        // Работа всё-таки закончилась: готовый результат отдаётся, а не теряется из-за срока.
        release.send(()).expect("отпускаем");
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut last = job.poll();
        while matches!(last, JobPoll::TimedOut { .. }) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
            last = job.poll();
        }
        assert_eq!(last, JobPoll::Done(7));
    }
}
