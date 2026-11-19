//! Защита от мерцания ламп: ручная политика, без автодетекта (решение 77, сессия 08c).
//!
//! Лампы на сети мерцают с удвоенной частотой сети; выдержка, кратная полупериоду
//! (10 мс при 50 Гц, 8,33 мс при 60 Гц), собирает целое число вспышек — полос на кадре нет.
//!
//! **Почему нет автоопределения.** «Детектор мерцания» HAL (`android.statistics.sceneFlicker`)
//! на этом телефоне фиктивный: на ~99,9 % кадров днём и под питаемым от батареи постоянным
//! LED-светом (физически без мерцания), при `AE_MODE` OFF и ON, он отвечает `60HZ`. Вендорский
//! тег `AntiBandingMode` всегда равен 120 — это эхо настройки «сеть 60 Гц», а не измерение.
//! Решение 73 («защита следует за детектором HAL») снято решением 77: защитой управляет
//! только человек — политика `AlwaysProtect`/`NeverProtect` (по умолчанию защита выключена) и
//! частота сети 50/60 Гц из конфига.
//! Сырое значение HAL по-прежнему сохраняется в `scene_flicker` результата для диагностики,
//! но на план оно не влияет.
//!
//! **Бэклог:** собственное автоопределение (кандидат — детектор по построчной статистике RAW
//! на GPU: полосы мерцания видны как периодическая яркость по строкам).

use std::time::Duration;

use camera_contract::three_a::MainsFrequency;

use super::config::FlickerPolicy;

/// Неизменяемая защита от мерцания для плана: владелец ответа «чему кратна выдержка».
///
/// Состояния нет: политику и частоту сети задаёт человек, поток их не меняет.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FlickerProtection {
    policy: FlickerPolicy,
    mains: MainsFrequency,
}

impl FlickerProtection {
    pub(super) fn new(policy: FlickerPolicy, mains: MainsFrequency) -> Self {
        Self { policy, mains }
    }

    /// Период кратности выдержки (полупериод сети); `None` — защита выключена вручную.
    pub(super) fn period(self) -> Option<Duration> {
        match self.policy {
            FlickerPolicy::AlwaysProtect => Some(half_mains_period(self.mains)),
            FlickerPolicy::NeverProtect => None,
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
