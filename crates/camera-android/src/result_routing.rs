//! Маршрут результатов кадров сессии: доставлять движку (основная камера) или только в поток
//! кадров (тёплая, 13b).
//!
//! # Зачем флаг, а не два вида контекстов
//! Роль сессии меняется на ходу: при обмене ролей тёплая становится основной, а основная —
//! тёплой, **не пересоздавая сессий**. Колбэки результатов NDK приходят на чужих потоках и не
//! могут ждать поток камеры, поэтому роль лежит в одном атомарном слове внутри контекста
//! результатов: колбэк читает её без блокировок, а поток камеры меняет одним `store`.
//!
//! # Гонка на границе обмена (осознанная)
//! Колбэк, прочитавший роль «основная» за мгновение до смены, может доставить свой результат
//! уже после возврата обмена: движок такие результаты бывшей основной отбрасывает со счётчиком
//! `standby_results_after_swap`. Обратной потери нет: результат новой основной после смены
//! доставляется сразу, а один результат, прочитанный как «тёплая» в момент смены, уходит только
//! в поток кадров (темпу движка один кадр не нужен).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use camera_core::backend_standby::StandbySlotId;

/// Роль сессии сейчас.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureRole {
    /// Основная камера: результаты идут движку.
    Primary,
    /// Тёплая камера в ячейке: результаты идут только в поток кадров.
    Standby(StandbySlotId),
}

/// Что делать с результатом успешного кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletedRoute {
    /// Доставить `CaptureCompleted` движку (и предложить потоку кадров).
    Engine,
    /// Только в поток кадров; `announce_streaming` — это первый результат сессии: сообщить
    /// движку `StandbyStreaming` (после предложения потоку, чтобы обмен нашёл результат на месте).
    Standby {
        slot: StandbySlotId,
        announce_streaming: bool,
    },
}

/// Что делать с провалом кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailedRoute {
    /// Доставить `CaptureFailed` движку.
    Engine,
    /// Не доставлять, посчитать в счётчиках ячейки.
    Standby { slot: StandbySlotId },
}

/// Роль сессии и признак «её готовность уже известна движку».
#[derive(Debug)]
pub struct ResultRouting {
    /// 0 — основная, `n + 1` — тёплая в ячейке `n`.
    encoded_role: AtomicU32,
    /// Хотя бы один результат этой сессии уже прошёл (в любой роли): `StandbyStreaming` ей не нужен.
    first_result_seen: AtomicBool,
}

fn encode(role: CaptureRole) -> u32 {
    match role {
        CaptureRole::Primary => 0,
        CaptureRole::Standby(slot) => u32::try_from(slot.index()).map_or(u32::MAX, |index| index.saturating_add(1)),
    }
}

fn decode(encoded: u32) -> CaptureRole {
    match encoded {
        0 => CaptureRole::Primary,
        // Номер ячейки — `u8`: значение, не влезающее в него, не создаётся `encode`.
        encoded => CaptureRole::Standby(StandbySlotId::new(u8::try_from(encoded - 1).unwrap_or(u8::MAX))),
    }
}

impl ResultRouting {
    /// Новая сессия с начальной ролью (первый результат ещё не был).
    pub fn new(role: CaptureRole) -> Self {
        Self {
            encoded_role: AtomicU32::new(encode(role)),
            first_result_seen: AtomicBool::new(false),
        }
    }

    /// Текущая роль.
    pub fn role(&self) -> CaptureRole {
        decode(self.encoded_role.load(Ordering::Acquire))
    }

    /// Повышение до основной: результаты идут движку с этого момента.
    pub fn promote_to_primary(&self) {
        self.encoded_role.store(encode(CaptureRole::Primary), Ordering::Release);
    }

    /// Понижение основной до тёплой в ячейке `slot`. Движок считает бывшую основную готовой
    /// сразу, поэтому `StandbyStreaming` для неё не нужен, даже если результатов ещё не было.
    pub fn demote_to_standby(&self, slot: StandbySlotId) {
        self.first_result_seen.store(true, Ordering::Release);
        self.encoded_role
            .store(encode(CaptureRole::Standby(slot)), Ordering::Release);
    }

    /// Куда направить результат успешного кадра. Отмечает, что первый результат прошёл.
    pub fn route_completed(&self) -> CompletedRoute {
        let was_first = !self.first_result_seen.swap(true, Ordering::AcqRel);
        match self.role() {
            CaptureRole::Primary => CompletedRoute::Engine,
            CaptureRole::Standby(slot) => CompletedRoute::Standby {
                slot,
                announce_streaming: was_first,
            },
        }
    }

    /// Куда направить провал кадра. Первый результат не отмечает: провал не доказывает, что
    /// камера стримит.
    pub fn route_failed(&self) -> FailedRoute {
        match self.role() {
            CaptureRole::Primary => FailedRoute::Engine,
            CaptureRole::Standby(slot) => FailedRoute::Standby { slot },
        }
    }
}

#[cfg(test)]
#[path = "result_routing_tests.rs"]
mod tests;
