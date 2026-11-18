//! Защита от мерцания ламп по детектору HAL (решение 08a, п. 2).
//!
//! Лампы на сети мерцают с удвоенной частотой сети; выдержка, кратная полупериоду
//! (10 мс при 50 Гц, 8,33 мс при 60 Гц), собирает целое число вспышек — полос на кадре нет.
//! Защита включена по умолчанию, а снять её можно только по слову детектора HAL
//! (`SceneFlicker::None` подряд `clear_after_frames` кадров). Любой отчёт о частоте сети
//! возвращает защиту сразу и уточняет саму частоту.
//!
//! Детектор — отдельный тип: он меняется только в `observe_result`/`reset`, а план читает
//! его **снимок** [`FlickerSnapshot`], который исполнитель забирает в `refresh()`.

use std::time::Duration;

use camera_contract::three_a::{MainsFrequency, SceneFlicker};

use super::config::FlickerPolicy;

/// Что план знает о мерцании: нужна ли защита и по какой частоте сети.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FlickerSnapshot {
    pub(super) protection: Protection,
    pub(super) mains: MainsFrequency,
}

/// Включена ли защита.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Protection {
    Active,
    Cleared,
}

impl FlickerSnapshot {
    /// Период кратности выдержки (полупериод сети); `None` — защита снята.
    pub(super) fn period(self) -> Option<Duration> {
        match self.protection {
            Protection::Active => Some(half_mains_period(self.mains)),
            Protection::Cleared => None,
        }
    }
}

/// Полупериод сети: 1 / (2·f), целые наносекунды (60 Гц → 8 333 333 нс, недолёт 0,33 нс).
pub(super) fn half_mains_period(mains: MainsFrequency) -> Duration {
    let hertz: u64 = match mains {
        MainsFrequency::Hz50 => 50,
        MainsFrequency::Hz60 => 60,
    };
    Duration::from_nanos(1_000_000_000 / (2 * hertz))
}

/// Детектор: считает кадры «мерцания нет» и помнит частоту сети.
#[derive(Debug, Clone, Copy)]
pub(super) struct FlickerDetector {
    policy: FlickerPolicy,
    /// Частота, которую назвал конфиг Android или (позже) детектор HAL.
    mains: MainsFrequency,
    protection: Protection,
    /// Сколько кадров подряд HAL сказал «мерцания нет».
    clear_streak: u32,
}

impl FlickerDetector {
    pub(super) fn new(policy: FlickerPolicy, mains: MainsFrequency) -> Self {
        Self {
            policy,
            mains,
            protection: Self::initial_protection(policy),
            clear_streak: 0,
        }
    }

    fn initial_protection(policy: FlickerPolicy) -> Protection {
        match policy {
            FlickerPolicy::NeverProtect => Protection::Cleared,
            FlickerPolicy::FollowDetector { .. } | FlickerPolicy::AlwaysProtect => Protection::Active,
        }
    }

    pub(super) fn snapshot(&self) -> FlickerSnapshot {
        FlickerSnapshot {
            protection: self.protection,
            mains: self.mains,
        }
    }

    /// Новый поток: защита снова по умолчанию, счёт кадров с нуля; частоту сети оставляем
    /// (она свойство места, а не потока).
    pub(super) fn reset_for_new_stream(&mut self) {
        self.protection = Self::initial_protection(self.policy);
        self.clear_streak = 0;
    }

    /// Учесть отчёт HAL об одном кадре; `true`, если изменился снимок для плана.
    /// `None` (ключа в результате нет) ничего не меняет — ни счёт, ни защиту.
    pub(super) fn observe(&mut self, reported: Option<SceneFlicker>) -> bool {
        let before = self.snapshot();
        match reported {
            None => {}
            Some(SceneFlicker::None) => self.observe_clear_frame(),
            Some(SceneFlicker::Mains50Hz) => self.observe_mains(MainsFrequency::Hz50),
            Some(SceneFlicker::Mains60Hz) => self.observe_mains(MainsFrequency::Hz60),
        }
        self.snapshot() != before
    }

    fn observe_clear_frame(&mut self) {
        let FlickerPolicy::FollowDetector { clear_after_frames } = self.policy else {
            return;
        };
        self.clear_streak = self.clear_streak.saturating_add(1);
        if self.clear_streak >= clear_after_frames {
            self.protection = Protection::Cleared;
        }
    }

    fn observe_mains(&mut self, frequency: MainsFrequency) {
        self.mains = frequency;
        self.clear_streak = 0;
        // `NeverProtect` — осознанный выбор «защиты нет»: даже явный отчёт её не включает.
        if self.policy != FlickerPolicy::NeverProtect {
            self.protection = Protection::Active;
        }
    }
}
