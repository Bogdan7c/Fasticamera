//! Приёмники службы: фазы (экран) и цели рендера.

use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use photo_output::service::{PrimaryTargetSink, RenderUnavailable, StillPhaseSink};
use still_contract::develop::PrimaryTargetOffer;
use still_contract::phase::StillPhase;
use still_contract::plan::StillId;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Собирает фазы и позволяет ждать нужную.
#[derive(Default)]
pub struct PhaseCollector {
    phases: Mutex<Vec<StillPhase>>,
    /// Момент прихода каждой фазы (тот же порядок, что у `phases`; пишется под замком `phases`).
    stamps: Mutex<Vec<Instant>>,
    changed: Condvar,
    /// Имена потоков, из которых приходили фазы.
    threads: Mutex<Vec<Option<String>>>,
    /// Пока `true`, `report_phase` не возвращается (изображает медленного получателя фаз).
    held: Mutex<bool>,
    released: Condvar,
}

impl StillPhaseSink for PhaseCollector {
    fn report_phase(&self, phase: StillPhase) {
        lock(&self.threads).push(std::thread::current().name().map(str::to_owned));
        {
            let mut phases = lock(&self.phases);
            phases.push(phase);
            lock(&self.stamps).push(Instant::now());
        }
        self.changed.notify_all();
        let mut held = lock(&self.held);
        while *held {
            held = self.released.wait(held).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

impl PhaseCollector {
    /// Следующие фазы будут «застревать» у получателя, пока не вызван `release`.
    pub fn hold(&self) {
        *lock(&self.held) = true;
    }

    pub fn release(&self) {
        *lock(&self.held) = false;
        self.released.notify_all();
    }

    pub fn thread_names(&self) -> Vec<Option<String>> {
        lock(&self.threads).clone()
    }

    /// Ждёт, пока придёт фаза `Secured` снимка.
    pub fn wait_secured(&self, id: StillId, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        let mut phases = lock(&self.phases);
        while !phases
            .iter()
            .any(|phase| matches!(phase, StillPhase::Secured { id: secured } if *secured == id))
        {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "фаза Secured снимка {id:?} не пришла за {timeout:?}");
            phases = self
                .changed
                .wait_timeout(phases, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Момент, когда пришла фаза `Secured` снимка (`None` — ещё не пришла).
    pub fn secured_at(&self, id: StillId) -> Option<Instant> {
        let phases = lock(&self.phases);
        let index = phases
            .iter()
            .position(|phase| matches!(phase, StillPhase::Secured { id: secured } if *secured == id))?;
        lock(&self.stamps).get(index).copied()
    }

    pub fn all(&self) -> Vec<StillPhase> {
        lock(&self.phases).clone()
    }

    /// Фазы одного снимка по порядку прихода.
    pub fn of(&self, id: StillId) -> Vec<StillPhase> {
        self.all().into_iter().filter(|phase| phase.id() == id).collect()
    }

    /// Ждёт итоговую (Saved/Failed) фазу снимка.
    pub fn wait_final(&self, id: StillId, timeout: Duration) -> StillPhase {
        let deadline = Instant::now() + timeout;
        let mut phases = lock(&self.phases);
        loop {
            if let Some(phase) = phases
                .iter()
                .find(|phase| phase.id() == id && !matches!(phase, StillPhase::Secured { .. }))
            {
                return phase.clone();
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "итоговая фаза снимка {id:?} не пришла за {timeout:?}");
            phases = self
                .changed
                .wait_timeout(phases, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// Цели, отданные рендеру. Может изображать недоступный рендер.
#[derive(Default)]
pub struct TargetCollector {
    offers: Mutex<Vec<PrimaryTargetOffer>>,
    /// Номера снимков в порядке выдачи предложений (не очищается при `wait_offer`).
    history: Mutex<Vec<StillId>>,
    changed: Condvar,
    reject_all: Mutex<bool>,
}

impl PrimaryTargetSink for TargetCollector {
    fn offer_primary_target(&self, offer: PrimaryTargetOffer) -> Result<(), RenderUnavailable> {
        if *lock(&self.reject_all) {
            return Err(RenderUnavailable);
        }
        lock(&self.history).push(offer.id());
        lock(&self.offers).push(offer);
        self.changed.notify_all();
        Ok(())
    }
}

impl TargetCollector {
    /// Рендер «остановлен»: предложения отвергаются.
    pub fn make_render_unavailable(&self) {
        *lock(&self.reject_all) = true;
    }

    /// Ждёт предложение для снимка и забирает его.
    pub fn wait_offer(&self, id: StillId, timeout: Duration) -> PrimaryTargetOffer {
        let deadline = Instant::now() + timeout;
        let mut offers = lock(&self.offers);
        loop {
            if let Some(index) = offers.iter().position(|offer| offer.id() == id) {
                return offers.remove(index);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "цель для снимка {id:?} не пришла за {timeout:?}");
            offers = self
                .changed
                .wait_timeout(offers, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// В каком порядке рендеру выдавали предложения.
    pub fn offer_order(&self) -> Vec<StillId> {
        lock(&self.history).clone()
    }

    pub fn count(&self) -> usize {
        lock(&self.offers).len()
    }
}
