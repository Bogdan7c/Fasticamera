//! Словарь нашего AE (сессия 08a, решения 63, 66, 68): что ходит между замером на потоке
//! рендера и движком камеры.
//!
//! Разделение ответственности:
//! - **поток рендера** (замер по статистике RAW, `scene-estimation` + `render-loop`) знает
//!   только сцену: сколько «выдержки × ISO» нужно этой сцене, чтобы средний серый лёг в
//!   середину нашего тона. Он не знает ни диапазонов камеры, ни мерцания, ни ручных значений;
//! - **движок камеры** (`camera-core`, исполнитель `ExposureController`) решает, как набрать
//!   это произведение: компенсация EV, блокировка, полуавтомат, делёж выдержка/ISO, кратность
//!   периоду мерцания, пределы камеры и кадра — и строит запрос HAL (`AE_MODE_OFF`).
//!
//! Между ними два ящика [`DirectiveSlot`] (последнее значение побеждает):
//! - [`ExposureProposalChannel`] (рендер → движок): [`ExposureProposal`] плюс «толчок» движку,
//!   который привязывается поздно ([`LateBoundRefreshSignal`]: рендер создаётся раньше камеры);
//! - `DirectiveSlot<ExposureDirective>` (движок → рендер): где мерить, профиль, блокировка,
//!   кто ведёт экспозицию и что реально стоит в запросе ([`AppliedExposure`]) — по нему рендер
//!   считает остаточную цифровую подтяжку ISP, пока сенсор догоняет.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde::Serialize;
use thiserror::Error;

use crate::capture_result::SensorTimestamp;
use crate::control::{ExposureBias, ExposureTime, Iso};
use crate::three_a::{AeAfLock, DirectiveGeneration, DirectiveSlot, PointOfInterest, PointRequestId, ThreeAProfile};

/// Почему произведение экспозиции отвергнуто.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum InvalidExposureProduct {
    #[error("произведение экспозиции {seconds_iso} с·ISO: нужно конечное и положительное")]
    NotPositiveFinite { seconds_iso: f64 },
}

/// «Выдержка × ISO» в секундах·ISO — сколько света набирает сенсор при данной оптике.
///
/// Это единая валюта AE: замер говорит «сцене нужно P», движок раскладывает P на выдержку
/// и ISO. Удвоение P — ровно +1 EV на снимке (сенсор линейный до насыщения). Значение всегда
/// конечное и положительное: ноль или NaN сюда не попадают, поэтому `log2` безопасен.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ExposureProduct(f64);

impl ExposureProduct {
    pub fn from_seconds_iso(seconds_iso: f64) -> Result<Self, InvalidExposureProduct> {
        if !(seconds_iso.is_finite() && seconds_iso > 0.0) {
            return Err(InvalidExposureProduct::NotPositiveFinite { seconds_iso });
        }
        Ok(Self(seconds_iso))
    }

    /// Произведение из значений запроса (оба уже проверены на ноль своими типами).
    pub fn from_sensor(exposure_time: ExposureTime, iso: Iso) -> Self {
        Self(exposure_time.duration().as_secs_f64() * f64::from(iso.value()))
    }

    /// Произведение из результата HAL; `None`, если выдержка или ISO нулевые (HAL не отдал
    /// честного значения — подставлять ничего нельзя).
    pub fn from_result(exposure_time: Duration, sensitivity_iso: u32) -> Option<Self> {
        Self::from_seconds_iso(exposure_time.as_secs_f64() * f64::from(sensitivity_iso)).ok()
    }

    pub fn seconds_iso(self) -> f64 {
        self.0
    }

    /// На сколько EV `self` больше `other` (`log2(self / other)`; отрицательное — меньше).
    pub fn ev_above(self, other: ExposureProduct) -> f64 {
        (self.0 / other.0).log2()
    }

    /// Произведение, умноженное на `2^ev`; `None`, если результат вышел за конечные числа.
    pub fn scaled_by_ev(self, ev: f64) -> Option<Self> {
        Self::from_seconds_iso(self.0 * ev.exp2()).ok()
    }
}

/// По какой области сделан замер, из которого выросло предложение.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "request")]
pub enum MeteredArea {
    /// Матричный замер по всему кадру.
    WholeFrame,
    /// Точечный замер по региону тапа с этим номером.
    Point(PointRequestId),
}

impl MeteredArea {
    /// Какую область замера требует точка интереса из указания.
    pub fn for_point(point: PointOfInterest) -> Self {
        match point {
            PointOfInterest::WholeFrame => MeteredArea::WholeFrame,
            PointOfInterest::Point { request, .. } => MeteredArea::Point(request),
        }
    }
}

/// Где замер на своём пути к цели.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalConvergence {
    /// Цель ещё движется к оценке сцены (или оценка ненадёжна).
    Searching,
    /// Цель совпала с оценкой сцены в пределах допуска профиля.
    Settled,
    /// Блокировка: замер дошёл до цели в нужной области и заморозил её. Дальше цель не
    /// меняется, пока движок не снимет блокировку.
    Holding,
}

/// Насколько надёжен замер последнего кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementQuality {
    /// Сигнал в рабочем диапазоне: цель посчитана по яркости сцены напрямую.
    Measured,
    /// Значимая часть области пересвечена: яркость известна только снизу, цель уменьшена
    /// ограниченным шагом (а не «точно»).
    ClippedHighlights,
    /// Область почти чёрная: яркость известна только сверху, цель увеличена ограниченным шагом.
    TooDark,
}

/// Предложение нашего AE: сколько света нужно сцене (рендер → движок).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ExposureProposal {
    /// Кадр, по статистике которого сделано предложение (`SENSOR_TIMESTAMP`). По метке движок
    /// отличает предложения прошлого потока от нового: метки одной шкалы `BOOTTIME` растут.
    pub measured_frame: SensorTimestamp,
    /// Произведение для среднего серого при компенсации 0 EV на текущей камере. Компенсацию,
    /// пределы и делёж добавляет движок.
    pub target: ExposureProduct,
    pub metered_area: MeteredArea,
    pub convergence: ProposalConvergence,
    pub quality: MeasurementQuality,
}

/// Кто ведёт экспозицию — по ручным параметрам намерения.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExposureAuthority {
    /// Выдержку и ISO ведёт наш AE.
    Automatic,
    /// Одна величина ручная, вторую ведёт наш AE (полуавтомат, решение 63).
    SemiAutomatic,
    /// Обе ручные: замер ничего не решает.
    Manual,
}

/// Что реально стоит в повторяющемся запросе HAL.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct AppliedExposure {
    /// Произведение «выдержка × ISO» в запросе.
    pub product: ExposureProduct,
    /// На сколько EV запрос отличается от того, что просили (цель × компенсация) при его
    /// построении: 0 — достигнуто, < 0 — не хватает пределов (честно темнее), > 0 — светлее
    /// нужного (короче выдержки сенсор не умеет). Для ручной экспозиции всегда 0.
    pub shortfall_ev: f32,
}

/// Указание нашему AE на потоке рендера (движок → рендер).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ExposureDirective {
    /// Где мерить: весь кадр или регион тапа.
    pub metering: PointOfInterest,
    /// Блокировка AE/AF: рендер досчитывает замер в области и замораживает цель.
    pub lock: AeAfLock,
    pub profile: ThreeAProfile,
    pub authority: ExposureAuthority,
    /// Компенсация EV пользователя (её применяет движок к запросу; рендеру она нужна для
    /// цели показа во время догоняния сенсора).
    pub bias: ExposureBias,
    /// Что стоит в запросе; `None`, пока повторяющегося запроса нет.
    pub applied: Option<AppliedExposure>,
}

impl ExposureDirective {
    /// Указание по умолчанию, пока движок ничего не опубликовал: автомат по всему кадру.
    pub const DEFAULT: ExposureDirective = ExposureDirective {
        metering: PointOfInterest::WholeFrame,
        lock: AeAfLock::Unlocked,
        profile: ThreeAProfile::Photo,
        authority: ExposureAuthority::Automatic,
        bias: ExposureBias::NONE,
        applied: None,
    };
}

/// Получатель «толчка»: «загляни в ящик, там новое». Реализует движок камеры
/// (`camera-core::ThreeAWaker`); толчок не блокирует и сливается с соседними.
pub trait RefreshSignal: Send + Sync {
    fn signal(&self);
}

/// Итог отправки толчка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalDelivery {
    /// Получатель привязан и толчок передан.
    Delivered,
    /// Получателя ещё нет (камера не стартовала): толчок посчитан и пропущен. Ящик при этом
    /// уже содержит значение, а `bind` досылает получателю один толчок, чтобы он его прочёл.
    NotBoundYet,
}

/// Толчок с поздней привязкой получателя.
///
/// Поток рендера создаётся раньше движка камеры, а `ThreeAWaker` появляется только после
/// запуска движка. Композиция отдаёт рендеру этот объект сразу, а получателя привязывает,
/// когда движок поднят. Клоны делят одно состояние.
#[derive(Clone, Default)]
pub struct LateBoundRefreshSignal {
    shared: Arc<SignalShared>,
}

#[derive(Default)]
struct SignalShared {
    receiver: Mutex<Option<Arc<dyn RefreshSignal>>>,
    /// Всего пропущено за всё время (диагностика, не сбрасывается).
    missed_before_bind: AtomicU64,
    /// Пропущено с последней привязки: если не ноль, `bind` досылает получателю один толчок.
    missed_since_last_bind: AtomicU64,
}

impl std::fmt::Debug for LateBoundRefreshSignal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LateBoundRefreshSignal")
            .field("bound", &self.is_bound())
            .field("missed_before_bind", &self.missed_before_bind())
            .finish()
    }
}

impl LateBoundRefreshSignal {
    pub fn new() -> Self {
        Self::default()
    }

    /// Привязать получателя (повторная привязка заменяет прежнего: перезапуск движка).
    ///
    /// Если с прошлой привязки толчки пропускались (`NotBoundYet`), новому получателю сразу
    /// уходит **один** толчок: иначе значение, лежащее в ящике, никто бы не прочёл — движок
    /// заглядывает в ящик только по толчку. Без пропусков лишнего толчка нет.
    pub fn bind(&self, receiver: Arc<dyn RefreshSignal>) {
        // Отравленный замок безопасен: внутри только `Option<Arc>`, частично записанным он
        // быть не может.
        *self.shared.receiver.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&receiver));
        if self.shared.missed_since_last_bind.swap(0, Ordering::AcqRel) > 0 {
            // Вызов получателя — вне замка (см. `signal`).
            receiver.signal();
        }
    }

    /// Отвязать получателя (движок остановлен): дальнейшие толчки — `NotBoundYet`.
    pub fn unbind(&self) {
        *self.shared.receiver.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    pub fn is_bound(&self) -> bool {
        self.shared
            .receiver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// Толкнуть получателя. Сам вызов получателя — вне замка: получатель может быть медленным,
    /// а привязка из другого потока не должна его ждать.
    pub fn signal(&self) -> SignalDelivery {
        let receiver = self
            .shared
            .receiver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        match receiver {
            Some(receiver) => {
                receiver.signal();
                SignalDelivery::Delivered
            }
            None => {
                self.shared.missed_before_bind.fetch_add(1, Ordering::Relaxed);
                self.shared.missed_since_last_bind.fetch_add(1, Ordering::AcqRel);
                SignalDelivery::NotBoundYet
            }
        }
    }

    /// Сколько толчков пропущено, пока получателя не было (диагностика).
    pub fn missed_before_bind(&self) -> u64 {
        self.shared.missed_before_bind.load(Ordering::Relaxed)
    }
}

/// Ящик предложений AE с толчком (рендер → движок). Писатель — поток рендера, читатель —
/// исполнитель экспозиции на потоке камеры. Клоны делят одно состояние.
#[derive(Debug, Clone, Default)]
pub struct ExposureProposalChannel {
    slot: DirectiveSlot<ExposureProposal>,
    signal: LateBoundRefreshSignal,
}

impl ExposureProposalChannel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Положить предложение и толкнуть движок.
    pub fn publish(&self, proposal: ExposureProposal) -> (DirectiveGeneration, SignalDelivery) {
        let generation = self.slot.publish(proposal);
        (generation, self.signal.signal())
    }

    /// Самое свежее предложение с поколением.
    pub fn latest(&self) -> Option<(DirectiveGeneration, ExposureProposal)> {
        self.slot.latest()
    }

    /// Предложение новее `seen`.
    pub fn newer_than(&self, seen: Option<DirectiveGeneration>) -> Option<(DirectiveGeneration, ExposureProposal)> {
        self.slot.newer_than(seen)
    }

    /// Толчок для привязки получателя композицией.
    pub fn refresh_signal(&self) -> &LateBoundRefreshSignal {
        &self.signal
    }
}

#[cfg(test)]
#[path = "auto_exposure_tests.rs"]
mod tests;
