//! Fake-датчик ориентации для тестов (сессия 12b): считает запуски и ЖИВЫЕ подписки.
//!
//! Подписка держит датчик, пока жива (`OrientationSubscription` с охранником остановки). Тест по счётчику живых
//! подписок видит, кто и когда держит датчик: экран камеры — пока он стоит; сценарий — на своё время.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use shell_contract::motion::OrientationSubscription;

/// Сколько показаний акселерометра «принято» у каждой подписки (как прежний fake оболочки).
const ACCELEROMETER_SAMPLES_SEEN: u64 = 25;

/// Запуски и живые подписки датчика.
#[derive(Default)]
pub struct FakeOrientationSensor {
    starts: AtomicU64,
    alive: Arc<AtomicU64>,
}

impl FakeOrientationSensor {
    /// Новая подписка: запуск засчитан, живых +1 до сброса подписки.
    pub fn subscribe(&self) -> OrientationSubscription {
        self.starts.fetch_add(1, Ordering::Relaxed);
        self.alive.fetch_add(1, Ordering::Relaxed);
        OrientationSubscription::new(
            // Датчик «жив»: сценарий `motion` судит по числу принятых показаний акселерометра.
            Arc::new(AtomicU64::new(ACCELEROMETER_SAMPLES_SEEN)),
            Box::new(AliveGuard(Arc::clone(&self.alive))),
        )
    }

    /// Сколько раз датчик запускали.
    pub fn starts(&self) -> u64 {
        self.starts.load(Ordering::Relaxed)
    }

    /// Сколько подписок живо сейчас.
    pub fn alive(&self) -> u64 {
        self.alive.load(Ordering::Relaxed)
    }
}

/// Охранник подписки: при сбросе живых −1 (как остановка настоящего потока датчика).
struct AliveGuard(Arc<AtomicU64>);

impl Drop for AliveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}
