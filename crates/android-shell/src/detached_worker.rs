//! Фоновый поток, остановка которого не блокирует вызывающего (нейтральная часть, тесты на ПК).
//!
//! Зачем: поток опроса батареи `fasticamera-power` может в момент остановки сидеть в binder-чтении
//! (сотни миллисекунд и дольше на занятом system_server). Останавливает его маршрутизатор, а
//! маршрутизатор ждать не должен. Поэтому остановка — это только сигнал: поток отсоединяется и
//! выходит сам, когда закончит текущее чтение.
//!
//! # Флаг «опрос занят»
//! Пока поток жив, второй запуск опроса обязан честно получить «уже идёт опрос», а не завести
//! второй поток рядом с ещё не вышедшим первым. Поэтому флаг занятости освобождает САМ поток,
//! когда действительно завершился: [`ClaimRelease`] переезжает в поток и срабатывает при его
//! выходе (обычном или по панике — деструкторы при раскрутке выполняются). Не запустился поток —
//! замыкание с [`ClaimRelease`] уничтожается сразу, и флаг освобождается тут же.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

/// Освобождает занятый ресурс (флаг «опрос идёт») ровно один раз — при уничтожении.
pub struct ClaimRelease<Release: FnOnce()> {
    release: Option<Release>,
}

impl<Release: FnOnce()> ClaimRelease<Release> {
    /// `release` вызовется при уничтожении значения (в потоке, который им владеет).
    pub fn new(release: Release) -> Self {
        Self { release: Some(release) }
    }
}

impl<Release: FnOnce()> Drop for ClaimRelease<Release> {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            release();
        }
    }
}

/// Сигнал остановки на стороне владельца: уничтожение = «остановись». Ничего не ждёт.
pub struct StopSignal {
    /// Уничтожение отправителя будит поток (`Disconnected`).
    _sender: Sender<()>,
}

/// Чем кончилось ожидание потока между итерациями.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopWait {
    /// Время ожидания вышло: работать дальше.
    TimedOut,
    /// Владелец попросил остановиться (или его уже нет).
    StopRequested,
}

/// Приёмник сигнала остановки на стороне потока.
pub struct StopWaiter {
    receiver: Receiver<()>,
}

impl StopWaiter {
    /// Ждёт не дольше `timeout`: либо время вышло, либо пришла остановка.
    pub fn wait(&self, timeout: Duration) -> StopWait {
        match self.receiver.recv_timeout(timeout) {
            Err(RecvTimeoutError::Timeout) => StopWait::TimedOut,
            // Отдельного сообщения никто не шлёт; отключение отправителя — та же остановка.
            Ok(()) | Err(RecvTimeoutError::Disconnected) => StopWait::StopRequested,
        }
    }
}

/// Запускает поток `name` с телом `body`; владелец получает [`StopSignal`], уничтожение которого
/// не ждёт выхода потока (поток отсоединён). `claim` уходит в поток и освобождается, когда поток
/// действительно завершился (или сразу, если поток не создан).
pub fn spawn_detached_worker<Release, Body>(
    name: &str,
    claim: ClaimRelease<Release>,
    body: Body,
) -> std::io::Result<StopSignal>
where
    Release: FnOnce() + Send + 'static,
    Body: FnOnce(&StopWaiter) + Send + 'static,
{
    let (sender, receiver) = mpsc::channel::<()>();
    let waiter = StopWaiter { receiver };
    // `JoinHandle` не сохраняем: остановка не ждёт поток (см. модуль). Паника тела не теряется
    // молча — её печатает стандартный обработчик паник процесса, а флаг освобождает раскрутка.
    thread::Builder::new().name(name.to_owned()).spawn(move || {
        // Порядок: сначала тело, потом освобождение флага — `claim` уничтожается последним.
        let _claim = claim;
        body(&waiter);
    })?;
    Ok(StopSignal { _sender: sender })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    use super::*;

    /// Флаг занятости, как `ShellRuntime::try_claim_sensor`/`release_sensor`.
    #[derive(Default)]
    struct ClaimFlag(AtomicBool);

    impl ClaimFlag {
        fn try_claim(&self) -> bool {
            self.0
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        }

        fn is_claimed(&self) -> bool {
            self.0.load(Ordering::Acquire)
        }

        fn release(&self) {
            self.0.store(false, Ordering::Release);
        }
    }

    fn release_of(flag: &Arc<ClaimFlag>) -> ClaimRelease<impl FnOnce() + Send + 'static> {
        let flag = Arc::clone(flag);
        ClaimRelease::new(move || flag.release())
    }

    /// Поток «завис» в чтении: остановка возвращается сразу, флаг держится, пока поток не вышел,
    /// повторный захват всё это время честно отказывает; после выхода — освобождён.
    #[test]
    fn stop_does_not_wait_for_a_blocked_worker_and_the_claim_is_released_by_the_worker_itself() {
        let flag = Arc::new(ClaimFlag::default());
        assert!(flag.try_claim());
        let (entered_tx, entered_rx) = mpsc::channel::<()>();
        let (unblock_tx, unblock_rx) = mpsc::channel::<()>();
        let (exited_tx, exited_rx) = mpsc::channel::<()>();
        let stop = spawn_detached_worker("test-detached", release_of(&flag), move |waiter| {
            entered_tx.send(()).expect("тест ждёт");
            // «binder-чтение»: блокируется, пока тест не отпустит.
            unblock_rx.recv().expect("тест отпустит");
            assert_eq!(waiter.wait(Duration::from_secs(5)), StopWait::StopRequested);
            exited_tx.send(()).expect("тест ждёт");
        })
        .expect("поток создан");
        entered_rx.recv().expect("поток вошёл в чтение");

        let started = Instant::now();
        drop(stop);
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "остановка не ждёт потока: {:?}",
            started.elapsed()
        );
        assert!(flag.is_claimed(), "поток ещё жив — флаг занят");
        assert!(
            !flag.try_claim(),
            "быстрый перезапуск получает «уже идёт опрос», а не второй поток"
        );

        unblock_tx.send(()).expect("поток ждёт");
        exited_rx.recv().expect("поток увидел остановку");
        let deadline = Instant::now() + Duration::from_secs(5);
        while flag.is_claimed() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(!flag.is_claimed(), "вышедший поток освободил флаг");
        assert!(flag.try_claim(), "после выхода потока опрос можно запустить снова");
    }

    /// Ожидание между итерациями: таймаут без сигнала, остановка — по уничтожению сигнала.
    #[test]
    fn waiter_distinguishes_timeout_from_stop() {
        let (sender, receiver) = mpsc::channel::<()>();
        let waiter = StopWaiter { receiver };
        assert_eq!(waiter.wait(Duration::from_millis(1)), StopWait::TimedOut);
        drop(StopSignal { _sender: sender });
        assert_eq!(waiter.wait(Duration::from_secs(5)), StopWait::StopRequested);
    }

    /// Освобождение срабатывает ровно один раз и только при уничтожении.
    #[test]
    fn claim_release_fires_once_on_drop() {
        let flag = Arc::new(ClaimFlag::default());
        assert!(flag.try_claim());
        let release = release_of(&flag);
        assert!(flag.is_claimed(), "до уничтожения флаг занят");
        drop(release);
        assert!(!flag.is_claimed());
    }
}
