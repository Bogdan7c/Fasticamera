//! Подсказки производительности системе (ADPF, `APerformanceHint`, сессия 07): нейтральный словарь.
//!
//! Зачем: планировщик частот телефона (DVFS) держит CPU и GPU «впритык» к текущей нагрузке. Кадр,
//! которому чуть-чуть не хватило частоты, опаздывает на vsync, и видоискатель теряет кадр камеры.
//! ADPF позволяет сказать системе: «эта группа потоков должна укладываться в `target` на кадр, вот
//! сколько реально заняли CPU и GPU» — и система поднимает частоты заранее, а не после промаха.
//!
//! Владение: потребитель (поток рендера) получает [`PerformanceHintService`] при запуске и сам
//! создаёт на своём потоке сессию [`FrameWorkSession`] — так же, как подписку vsync из
//! `VsyncSource`. Сессия закрывается в `Drop` реализации. Реализация для телефона — в
//! `android-shell`; на ПК — [`NoPerformanceHints`] (честное «не поддерживается») или подделка
//! `fake::FakePerformanceHints` (фича `fake-performance-hints`).
//!
//! Ошибки не сливаются в `bool`: «платформа не умеет» ([`PerformanceHintError::Unsupported`]),
//! «вызов не удался» ([`PerformanceHintError::Failed`]) и «сами передали неверные числа»
//! ([`PerformanceHintError::InvalidWork`]) потребитель считает по-разному.

use std::time::Duration;

use crate::clock::MonotonicNanos;

/// Идентификатор потока ОС (`gettid()` на Android), который входит в сессию подсказок.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HintThreadId(i32);

impl HintThreadId {
    pub const fn from_raw(tid: i32) -> Self {
        Self(tid)
    }

    pub const fn as_raw(self) -> i32 {
        self.0
    }
}

/// Фактическая работа одного кадра для отчёта системе.
///
/// Инварианты платформы (`APerformanceHint_reportActualWorkDuration2`) проверяются при создании:
/// начало периода и общая длительность больше нуля, хотя бы одна из длительностей CPU/GPU больше
/// нуля. GPU `None` — «не измерено» (платформа получает ноль, что по документации и значит
/// «не мерили»).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameWork {
    period_start: MonotonicNanos,
    total: Duration,
    cpu: Duration,
    gpu: Option<Duration>,
}

/// Почему числа кадра не годятся для отчёта.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidFrameWork {
    #[error("начало периода работы должно быть больше нуля")]
    NonPositiveStart,
    #[error("общая длительность работы должна быть больше нуля")]
    ZeroTotal,
    #[error("хотя бы одна из длительностей CPU или GPU должна быть больше нуля")]
    NothingMeasured,
}

impl FrameWork {
    /// Проверяет числа по правилам платформы (см. тип).
    pub fn new(
        period_start: MonotonicNanos,
        total: Duration,
        cpu: Duration,
        gpu: Option<Duration>,
    ) -> Result<Self, InvalidFrameWork> {
        if period_start.as_nanos() <= 0 {
            return Err(InvalidFrameWork::NonPositiveStart);
        }
        if total.is_zero() {
            return Err(InvalidFrameWork::ZeroTotal);
        }
        if cpu.is_zero() && gpu.is_none_or(|gpu| gpu.is_zero()) {
            return Err(InvalidFrameWork::NothingMeasured);
        }
        Ok(Self {
            period_start,
            total,
            cpu,
            gpu,
        })
    }

    /// Начало периода работы (`CLOCK_MONOTONIC`).
    pub fn period_start(&self) -> MonotonicNanos {
        self.period_start
    }

    /// Общая длительность работы кадра.
    pub fn total(&self) -> Duration {
        self.total
    }

    /// Время CPU кадра.
    pub fn cpu(&self) -> Duration {
        self.cpu
    }

    /// Время GPU кадра (`None` — не измерено).
    pub fn gpu(&self) -> Option<Duration> {
        self.gpu
    }
}

/// Отказ службы или сессии подсказок.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PerformanceHintError {
    /// Платформа (или это устройство) подсказки не поддерживает: продолжать без них.
    #[error("подсказки производительности не поддерживаются: {reason}")]
    Unsupported { reason: String },
    /// Вызов платформы вернул ошибку (`EINVAL`, `EPIPE`…).
    #[error("{operation} вернул код {code}")]
    Failed { operation: &'static str, code: i32 },
    /// Переданы числа, которые платформа не примет.
    #[error("неверный отчёт о работе: {0}")]
    InvalidWork(#[from] InvalidFrameWork),
}

/// Действующая сессия подсказок для группы потоков. Закрывается в `Drop` реализации.
///
/// Вызывается только с потока-владельца (поток рендера); `Send` нужен, потому что объект
/// потребителя создаётся на одном потоке и переезжает в свой.
pub trait FrameWorkSession: Send {
    /// Новая целевая длительность кадра (например, после смены частоты экрана).
    fn update_target(&mut self, target: Duration) -> Result<(), PerformanceHintError>;

    /// Отчёт о фактической работе одного кадра. Неблокирующий (у Android — односторонний binder).
    fn report_frame(&mut self, work: &FrameWork) -> Result<(), PerformanceHintError>;
}

/// Служба подсказок производительности.
pub trait PerformanceHintService: Send + Sync {
    /// Идентификатор ВЫЗЫВАЮЩЕГО потока (его потребитель кладёт в сессию).
    fn current_thread(&self) -> Result<HintThreadId, PerformanceHintError>;

    /// Создаёт сессию для потоков `threads` с целевой длительностью кадра `target`.
    fn create_session(
        &self,
        threads: &[HintThreadId],
        target: Duration,
    ) -> Result<Box<dyn FrameWorkSession>, PerformanceHintError>;
}

/// Служба «подсказок нет» (ПК, запасной путь): любое действие — `Unsupported` с причиной.
#[derive(Debug, Clone)]
pub struct NoPerformanceHints {
    reason: &'static str,
}

impl NoPerformanceHints {
    /// `reason` попадёт в диагностику потребителя («ПК», «оболочка не поднялась»…).
    pub const fn new(reason: &'static str) -> Self {
        Self { reason }
    }

    fn unsupported(&self) -> PerformanceHintError {
        PerformanceHintError::Unsupported {
            reason: self.reason.to_owned(),
        }
    }
}

impl PerformanceHintService for NoPerformanceHints {
    fn current_thread(&self) -> Result<HintThreadId, PerformanceHintError> {
        Err(self.unsupported())
    }

    fn create_session(
        &self,
        _threads: &[HintThreadId],
        _target: Duration,
    ) -> Result<Box<dyn FrameWorkSession>, PerformanceHintError> {
        Err(self.unsupported())
    }
}

#[cfg(any(test, feature = "fake-performance-hints"))]
pub mod fake;

#[cfg(test)]
mod tests {
    use super::*;

    const START: MonotonicNanos = MonotonicNanos::from_nanos(1_000);
    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn frame_work_follows_platform_rules() {
        assert_eq!(
            FrameWork::new(MonotonicNanos::from_nanos(0), MS, MS, None),
            Err(InvalidFrameWork::NonPositiveStart)
        );
        assert_eq!(
            FrameWork::new(START, Duration::ZERO, MS, None),
            Err(InvalidFrameWork::ZeroTotal)
        );
        assert_eq!(
            FrameWork::new(START, MS, Duration::ZERO, None),
            Err(InvalidFrameWork::NothingMeasured)
        );
        assert_eq!(
            FrameWork::new(START, MS, Duration::ZERO, Some(Duration::ZERO)),
            Err(InvalidFrameWork::NothingMeasured)
        );
        let gpu_only = FrameWork::new(START, MS, Duration::ZERO, Some(MS)).expect("одного GPU достаточно");
        assert_eq!((gpu_only.cpu(), gpu_only.gpu()), (Duration::ZERO, Some(MS)));
        let full = FrameWork::new(START, 3 * MS, MS, Some(2 * MS)).expect("полный отчёт");
        assert_eq!(
            (full.period_start(), full.total(), full.cpu(), full.gpu()),
            (START, 3 * MS, MS, Some(2 * MS))
        );
    }

    #[test]
    fn absent_service_is_honestly_unsupported() {
        let service = NoPerformanceHints::new("ПК");
        let expected = PerformanceHintError::Unsupported {
            reason: "ПК".to_owned(),
        };
        assert_eq!(service.current_thread(), Err(expected.clone()));
        assert_eq!(
            service.create_session(&[HintThreadId::from_raw(7)], MS).err(),
            Some(expected)
        );
    }
}
