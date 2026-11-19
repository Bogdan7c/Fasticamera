//! Индикатор фокуса и блокировка AE/AF.
//!
//! Индикатор (жёлтый квадрат) — состояние экрана: где он, откуда взялся и с какого момента показан.
//! Блокировка — двухступенчатая: долгий тап отправляет намерение и **ждёт подтверждения камеры**
//! ([`LockState::Pending`]); плашка «AE/AF LOCK» видна только при [`LockState::Locked`], то есть
//! пока камера сама говорит `Locked`.

use camera_contract::three_a::{PointRequestId, SensorPoint};
use serde::{Deserialize, Serialize};
use shell_contract::clock::MonotonicNanos;
use thiserror::Error;
use ui_core::geometry::Point;

use crate::facts::FocusPhase;
use crate::time::{deadline_after, has_reached};

/// Настройки индикатора.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FocusConfig {
    /// Сколько показывать квадрат после тапа без блокировки, мс. `[estimate]` 3 с — Apple время не
    /// публикует (на видео A квадрат гаснет через несколько секунд, подгонка, не Apple).
    pub indicator_visible_ms: u32,
    /// Сколько ждать подтверждения блокировки от камеры, мс. `[estimate]` 1,5 с — запас на
    /// фокусировку после долгого тапа; потом экран считает, что блокировки не будет.
    pub lock_confirmation_timeout_ms: u32,
}

impl Default for FocusConfig {
    fn default() -> Self {
        Self {
            indicator_visible_ms: 3_000,
            lock_confirmation_timeout_ms: 1_500,
        }
    }
}

/// Ошибка конфига индикатора.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("время {field} должно быть больше нуля")]
pub struct FocusConfigError {
    pub field: &'static str,
}

impl FocusConfig {
    pub fn validate(&self) -> Result<(), FocusConfigError> {
        if self.indicator_visible_ms == 0 {
            return Err(FocusConfigError {
                field: "indicator_visible_ms",
            });
        }
        if self.lock_confirmation_timeout_ms == 0 {
            return Err(FocusConfigError {
                field: "lock_confirmation_timeout_ms",
            });
        }
        Ok(())
    }
}

/// Откуда взялся индикатор.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusOrigin {
    /// Тап пользователя.
    Tap,
    /// Долгий тап (блокировка).
    Lock,
    /// Камера сама поставила точку (задел под фокус-трекинг, решение 69).
    Camera,
}

/// Индикатор фокуса.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum FocusIndicator {
    /// Не показан.
    #[default]
    Hidden,
    /// Показан в точке экрана (pt).
    Shown {
        at: Point,
        origin: FocusOrigin,
        /// С какого момента показан (от него считается затухание).
        since: MonotonicNanos,
    },
}

/// Состояние блокировки AE/AF.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LockState {
    #[default]
    Unlocked,
    /// Долгий тап отправлен, камера ещё не подтвердила; плашки нет.
    Pending { deadline: MonotonicNanos },
    /// Камера подтвердила блокировку: плашка «AE/AF LOCK» видна.
    Locked,
}

/// Допуск совпадения точки камеры с точкой, которую экран отправил, в долях активной области.
/// Камера обычно возвращает центр региона тем же (у края регион обрезается, а не сдвигается) с
/// точностью до пикселя сенсора (1/4096 ≈ 2,4e-4); 1e-3 — запас в несколько пикселей. Две точки
/// ближе допуска для камеры неразличимы, путаница между ними безвредна.
const CAMERA_POINT_MATCH_TOLERANCE: f32 = 1e-3;

/// Сколько вытесненных новым касанием точек помним, пока камера не подтвердила последнее.
/// Больше трёх неотвеченных касаний за время отклика камеры нереально; при переполнении самая
/// старая забывается (её запоздалый ответ тогда будет принят как «камера сама сдвинула точку»).
const SUPERSEDED_POINTS_CAPACITY: usize = 4;

/// Как точка, пришедшая от камеры, соотносится с ожиданием экрана.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CameraPointMatch {
    /// Экран ничего не ждёт: точка поставлена самой камерой (задел под трекинг), принимаем.
    NotAwaiting,
    /// Ответ на последнее касание экрана (в том числе с точкой, которую камера сама сдвинула):
    /// принимаем и перестаём ждать.
    AnswersLastTap,
    /// Запоздалый ответ на более раннее касание (экран уже отправил новое): игнорируем, чтобы
    /// квадрат не прыгал на старую точку и обратно.
    StaleForLastTap,
}

/// Точки сенсора, отправленные камере и затем вытесненные более новым касанием.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct SupersededPoints {
    points: [Option<SensorPoint>; SUPERSEDED_POINTS_CAPACITY],
    /// Куда писать следующую (кольцо: вытесняется самая старая).
    next_slot: usize,
}

impl SupersededPoints {
    fn push(&mut self, point: SensorPoint) {
        self.points[self.next_slot] = Some(point);
        self.next_slot = (self.next_slot + 1) % SUPERSEDED_POINTS_CAPACITY;
    }

    fn contains_near(&self, center: SensorPoint) -> bool {
        self.points.iter().flatten().any(|point| is_near(*point, center))
    }
}

fn is_near(first: SensorPoint, second: SensorPoint) -> bool {
    (first.x() - second.x()).abs() <= CAMERA_POINT_MATCH_TOLERANCE
        && (first.y() - second.y()).abs() <= CAMERA_POINT_MATCH_TOLERANCE
}

/// Исход проверки срока ожидания блокировки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockWaitCheck {
    /// Срок вышел: ожидание снято, блокировки не будет.
    Expired,
    /// Ждать нечего или срок ещё не вышел: состояние не тронуто.
    NotExpired,
}

/// Всё, что экран знает про фокус.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FocusState {
    pub(crate) indicator: FocusIndicator,
    pub(crate) lock: LockState,
    pub(crate) phase: FocusPhase,
    /// Точка сенсора, которую экран отправил последним тапом и ещё не увидел подтверждённой камерой.
    /// Пока она есть: устаревший факт «весь кадр» не гасит свежий квадрат, запоздалый ответ камеры
    /// на более ранний тап игнорируется, а блокировка не подтверждается (см. [`CameraPointMatch`]).
    /// Экран не знает номера запроса камеры (его выдаёт движок), поэтому сопоставляет по координатам.
    pub(crate) awaited_point: Option<SensorPoint>,
    /// Вытесненные новым касанием точки, ответ на которые ещё может прийти с опозданием.
    superseded_points: SupersededPoints,
    /// Номер последней точки камеры, которую экран уже учёл.
    pub(crate) last_seen_request: Option<PointRequestId>,
}

impl FocusState {
    pub fn indicator(&self) -> FocusIndicator {
        self.indicator
    }

    pub fn lock(&self) -> LockState {
        self.lock
    }

    pub fn phase(&self) -> FocusPhase {
        self.phase
    }

    /// Видна ли плашка «AE/AF LOCK»: только когда камера подтвердила блокировку.
    pub fn is_lock_plaque_visible(&self) -> bool {
        matches!(self.lock, LockState::Locked)
    }

    /// Показать индикатор в точке.
    pub(crate) fn show(&mut self, at: Point, origin: FocusOrigin, now: MonotonicNanos) {
        self.indicator = FocusIndicator::Shown { at, origin, since: now };
    }

    /// Скрыть индикатор и забыть ожидание подтверждения точки.
    pub(crate) fn hide(&mut self) {
        self.indicator = FocusIndicator::Hidden;
        self.awaited_point = None;
        self.superseded_points = SupersededPoints::default();
    }

    /// Экран отправил камере точку и ждёт, что камера её подтвердит.
    /// Предыдущая неподтверждённая точка (если была) становится «вытесненной».
    pub(crate) fn await_camera_point(&mut self, sent: SensorPoint) {
        if let Some(previous) = self.awaited_point.replace(sent) {
            self.superseded_points.push(previous);
        }
    }

    /// Ждём ли подтверждения последнего тапа.
    pub(crate) fn is_awaiting_camera_point(&self) -> bool {
        self.awaited_point.is_some()
    }

    /// Соотносит центр региона, пришедшего от камеры, с ожиданием. Состояние не меняет.
    ///
    /// Порядок проверки: сначала последнее касание, потом вытесненные; точка, не похожая ни на одну
    /// из отправленных, считается ответом на последнее касание (камера сама уточнила точку).
    pub(crate) fn match_camera_point(&self, center: SensorPoint) -> CameraPointMatch {
        let Some(sent) = self.awaited_point else {
            return CameraPointMatch::NotAwaiting;
        };
        if is_near(sent, center) || !self.superseded_points.contains_near(center) {
            CameraPointMatch::AnswersLastTap
        } else {
            CameraPointMatch::StaleForLastTap
        }
    }

    /// Камера подтвердила последний тап.
    pub(crate) fn confirm_camera_point(&mut self) {
        self.awaited_point = None;
        self.superseded_points = SupersededPoints::default();
    }

    /// Сдвинуть уже показанный индикатор (камера уточнила точку); происхождение и время показа не меняются.
    pub(crate) fn reposition(&mut self, new_at: Point) {
        if let FocusIndicator::Shown { origin, since, .. } = self.indicator {
            self.indicator = FocusIndicator::Shown {
                at: new_at,
                origin,
                since,
            };
        }
    }

    /// Продлить показ (пользователь двигает «солнце»): отсчёт затухания начинается заново.
    pub(crate) fn keep_alive(&mut self, now: MonotonicNanos) {
        if let FocusIndicator::Shown { at, origin, .. } = self.indicator {
            self.indicator = FocusIndicator::Shown { at, origin, since: now };
        }
    }

    /// Начать ждать подтверждения блокировки до `now + timeout`.
    pub(crate) fn begin_lock_wait(&mut self, now: MonotonicNanos, timeout_ms: u32) {
        self.lock = LockState::Pending {
            deadline: deadline_after(now, timeout_ms),
        };
    }

    /// Затухание по времени. Не гаснет при блокировке и пока ждём её подтверждения.
    /// Возвращает `true`, если индикатор погас.
    pub(crate) fn fade_if_due(&mut self, now: MonotonicNanos, visible_ms: u32) -> bool {
        let FocusIndicator::Shown { since, .. } = self.indicator else {
            return false;
        };
        if !matches!(self.lock, LockState::Unlocked) {
            return false;
        }
        if has_reached(now, deadline_after(since, visible_ms)) {
            self.hide();
            return true;
        }
        false
    }

    /// Вышло ли время ожидания подтверждения блокировки; если да — ожидание снимается.
    pub(crate) fn expire_lock_wait(&mut self, now: MonotonicNanos) -> LockWaitCheck {
        match self.lock {
            LockState::Pending { deadline } if has_reached(now, deadline) => {
                self.lock = LockState::Unlocked;
                LockWaitCheck::Expired
            }
            _ => LockWaitCheck::NotExpired,
        }
    }
}
