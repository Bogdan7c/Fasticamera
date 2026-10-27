//! Подставной источник питания для тестов потребителей (сценарий замера тока).
//!
//! Тест сам «читает батарею»: кладёт показания и сбои в кольцо активной подписки, как это
//! делал бы поток опроса на телефоне. Доступен в тестах `shell-contract` и через фичу
//! `fake-power-source` для тестов других crate-ов.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use super::BatterySample;
use super::sampling::{
    PowerPushOutcome, PowerReadFailure, PowerSampleRing, PowerSampling, PowerSamplingRequest, PowerSource,
    PowerSourceError,
};

/// Почему тест не смог подложить показание.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FakePowerError {
    #[error("опрос не запущен или подписка уже уничтожена")]
    NotSampling,
}

#[derive(Debug, Default)]
struct FakeState {
    /// Ошибка, которую вернёт следующий `start_sampling` (`None` — запуск успешен).
    start_error: Option<PowerSourceError>,
    /// Кольцо живой подписки: слабая ссылка, чтобы уничтожение подписки «останавливало опрос».
    active_ring: Weak<PowerSampleRing>,
    /// Последний принятый запрос (тест проверяет, что потребитель передал свой конфиг).
    last_request: Option<PowerSamplingRequest>,
}

/// Fake-источник: одна подписка за раз, как у настоящего.
#[derive(Debug, Default)]
pub struct FakePowerSource {
    state: Mutex<FakeState>,
}

impl FakePowerSource {
    pub fn new() -> Self {
        Self::default()
    }

    /// Следующий `start_sampling` вернёт эту ошибку (один раз).
    pub fn fail_next_start(&self, error: PowerSourceError) {
        self.lock_state().start_error = Some(error);
    }

    /// Кладёт показание в активную подписку.
    pub fn push_sample(&self, sample: BatterySample) -> Result<PowerPushOutcome, FakePowerError> {
        Ok(self.active_ring()?.push(sample))
    }

    /// Имитирует неудачное чтение платформы.
    pub fn fail_read(&self, failure: PowerReadFailure) -> Result<(), FakePowerError> {
        self.active_ring()?.record_failed_read(failure);
        Ok(())
    }

    pub fn is_sampling(&self) -> bool {
        self.lock_state().active_ring.strong_count() > 0
    }

    pub fn last_request(&self) -> Option<PowerSamplingRequest> {
        self.lock_state().last_request
    }

    fn active_ring(&self) -> Result<Arc<PowerSampleRing>, FakePowerError> {
        self.lock_state()
            .active_ring
            .upgrade()
            .ok_or(FakePowerError::NotSampling)
    }

    fn lock_state(&self) -> MutexGuard<'_, FakeState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl PowerSource for FakePowerSource {
    fn start_sampling(&self, request: PowerSamplingRequest) -> Result<PowerSampling, PowerSourceError> {
        let mut state = self.lock_state();
        if let Some(error) = state.start_error.take() {
            return Err(error);
        }
        if state.active_ring.strong_count() > 0 {
            return Err(PowerSourceError::AlreadySampling);
        }
        let ring = Arc::new(PowerSampleRing::new(request.ring_capacity));
        state.active_ring = Arc::downgrade(&ring);
        state.last_request = Some(request);
        // Потока опроса нет: остановкой служит уничтожение единственной сильной ссылки на кольцо.
        Ok(PowerSampling::new(ring, Box::new(())))
    }
}
