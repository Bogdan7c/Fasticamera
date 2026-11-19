//! Вектор гравитации — наклон телефона в нейтральном виде (сессия 11).
//!
//! Нужен блику материала стекла: направление света следует за наклоном телефона
//! (`docs/ios-camera/glass.md`, раздел 1, WWDC25 219, 11:29). Классификатор ориентации (решение 29)
//! сводит акселерометр к четырём положениям и наружу сырых показаний не отдаёт, а блику нужен
//! непрерывный вектор — поэтому отдельный словарь.
//!
//! Оси — как у `SensorEvent` Android в естественной (портретной) ориентации телефона:
//! x — вправо, y — вверх, z — из экрана к зрителю. Единицы — м/с². Телефон лежит экраном вверх →
//! вектор ≈ (0, 0, +9,81); стоит вертикально в портрете → ≈ (0, +9,81, 0).
//!
//! Время — шкала датчика `BOOTTIME`, а не `MONOTONIC` (UI и vsync): сглаживание по времени
//! делает потребитель, переводя шкалы явно (см. `clock`).

use std::sync::{Arc, Mutex, PoisonError};

use crate::clock::BoottimeNanos;

/// Вектор гравитации в осях телефона, м/с².
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GravityVector {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// Одно показание: вектор и время датчика.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GravitySample {
    pub time: BoottimeNanos,
    pub vector: GravityVector,
}

/// Что сейчас лежит в ящике гравитации.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GravityReading {
    /// Датчик ещё не прислал ни одного показания.
    NoSampleYet,
    /// Самое свежее показание и сколько всего показаний принято с запуска.
    Latest {
        sample: GravitySample,
        samples_received: u64,
    },
}

/// Ящик «самое свежее показание»: один писатель (поток датчика), любые читатели.
///
/// Блику нужна только текущая поза, история не нужна — поэтому не кольцо, как у гироскопа.
#[derive(Debug, Default)]
pub struct GravitySlot {
    state: Mutex<GravitySlotState>,
}

#[derive(Debug, Default)]
struct GravitySlotState {
    latest: Option<GravitySample>,
    samples_received: u64,
}

impl GravitySlot {
    pub fn new() -> Self {
        Self::default()
    }

    /// Положить новое показание (заменяет прежнее).
    pub fn publish(&self, sample: GravitySample) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.latest = Some(sample);
        state.samples_received = state.samples_received.saturating_add(1);
    }

    /// Прочитать самое свежее показание (без ожидания).
    pub fn read(&self) -> GravityReading {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        match state.latest {
            None => GravityReading::NoSampleYet,
            Some(sample) => GravityReading::Latest {
                sample,
                samples_received: state.samples_received,
            },
        }
    }
}

/// Откуда берётся вектор гравитации (для диагностики: путь выбирается на устройстве, и от него
/// зависит шум и задержка блика).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GravitySource {
    /// Источник не сообщил путь (значение по умолчанию у [`GravitySubscription::new`]).
    Unspecified,
    /// Готовый датчик гравитации платформы (`ASENSOR_TYPE_GRAVITY`).
    DedicatedSensor,
    /// Акселерометр с нашим низкочастотным фильтром (на телефоне нет датчика гравитации).
    FilteredAccelerometer,
    /// Подставленный извне источник (тесты, fake-оболочка).
    Injected,
}

/// Работающая подписка на гравитацию. Датчик выключается, когда подписка уничтожена.
pub struct GravitySubscription {
    slot: Arc<GravitySlot>,
    source: GravitySource,
    _stop_on_drop: Box<dyn Send>,
}

impl GravitySubscription {
    /// `stop_on_drop` — объект источника, чей `Drop` останавливает датчик.
    pub fn new(slot: Arc<GravitySlot>, stop_on_drop: Box<dyn Send>) -> Self {
        Self {
            slot,
            source: GravitySource::Unspecified,
            _stop_on_drop: stop_on_drop,
        }
    }

    /// Указывает, каким путём получен вектор (диагностика).
    #[must_use]
    pub fn with_source(mut self, source: GravitySource) -> Self {
        self.source = source;
        self
    }

    /// Каким путём получен вектор.
    pub fn source(&self) -> GravitySource {
        self.source
    }

    pub fn slot(&self) -> &Arc<GravitySlot> {
        &self.slot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(time_ns: i64, z: f32) -> GravitySample {
        GravitySample {
            time: BoottimeNanos::from_nanos(time_ns),
            vector: GravityVector { x: 0.0, y: 0.0, z },
        }
    }

    #[test]
    fn subscription_defaults_to_unspecified_source_and_exposes_the_chosen_one() {
        let slot = Arc::new(GravitySlot::new());
        let plain = GravitySubscription::new(Arc::clone(&slot), Box::new(()));
        assert_eq!(plain.source(), GravitySource::Unspecified);
        let tagged = plain.with_source(GravitySource::DedicatedSensor);
        assert_eq!(tagged.source(), GravitySource::DedicatedSensor);
        // Подписка отдаёт именно тот ящик, куда пишет источник.
        slot.publish(sample(7, 9.81));
        assert_eq!(
            tagged.slot().read(),
            GravityReading::Latest {
                sample: sample(7, 9.81),
                samples_received: 1
            }
        );
    }

    #[test]
    fn empty_slot_reports_no_sample_yet() {
        assert_eq!(GravitySlot::new().read(), GravityReading::NoSampleYet);
    }

    #[test]
    fn latest_sample_replaces_previous_and_counts_all() {
        let slot = GravitySlot::new();
        slot.publish(sample(1, 1.0));
        slot.publish(sample(2, 9.81));
        assert_eq!(
            slot.read(),
            GravityReading::Latest {
                sample: sample(2, 9.81),
                samples_received: 2
            }
        );
    }
}
