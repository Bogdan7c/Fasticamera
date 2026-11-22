//! Словарь автоматики 3A (сессия 08): намерения «куда фокусироваться и мерить», состояния
//! из результатов HAL, режим нашего AWB, возможности 3A камеры и серия с брекетингом (V10).
//!
//! Разделение ответственности (решения 9, 63–66):
//! - фокус — вендорский AF через HAL; экспозиция — наш AE (08a, в 08 временно вендорский);
//!   баланс белого — наш AWB на потоке рендера; всё решает один владелец — `camera-core`;
//! - здесь только нейтральные типы: ни Camera2, ни GPU.

use serde::Serialize;
use std::sync::{Arc, Mutex, PoisonError};
use thiserror::Error;

use crate::capabilities::Rational;
use crate::control::{ExposureTime, Iso, WhiteBalanceSetting};
use crate::flash::{FlashResult, FlashShotId};

/// Профиль автоматики (V6): скорость адаптации AE/AWB и допустимость скачков.
/// В 08 реализуется `Photo`, но каждый контракт принимает профиль с первого дня.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreeAProfile {
    /// Фото: быстрая сходимость, скачок после тапа допустим.
    Photo,
    /// Видео: медленная плавная адаптация, без скачков и «дыхания».
    Video,
}

/// Почему точка или регион замера отвергнуты.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum InvalidMeteringRegion {
    #[error("координата {value} вне [0, 1]")]
    CoordinateOutOfRange { value: f32 },
    #[error("размер региона {fraction} вне (0, 1]")]
    SizeOutOfRange { fraction: f32 },
}

/// Точка в нормированных координатах **активной области сенсора** в осях сенсора
/// (до поворота на экран): (0, 0) — левый верхний угол, (1, 1) — правый нижний.
/// Перевод из точки касания на экране (поворот, кадрирование) — забота экрана камеры (12).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SensorPoint {
    x: f32,
    y: f32,
}

impl SensorPoint {
    pub const CENTER: SensorPoint = SensorPoint { x: 0.5, y: 0.5 };

    pub fn new(x: f32, y: f32) -> Result<Self, InvalidMeteringRegion> {
        for value in [x, y] {
            if !(0.0..=1.0).contains(&value) {
                return Err(InvalidMeteringRegion::CoordinateOutOfRange { value });
            }
        }
        Ok(Self { x, y })
    }

    pub fn x(self) -> f32 {
        self.x
    }

    pub fn y(self) -> f32 {
        self.y
    }
}

/// Квадратный (в пикселях сенсора) регион замера/фокуса вокруг точки.
/// `size_fraction` — сторона как доля **меньшей** стороны активной области; регион у края
/// обрезается по границе, а не сдвигается (так точка тапа остаётся центром замера).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct MeteringRegion {
    center: SensorPoint,
    size_fraction: f32,
}

impl MeteringRegion {
    pub fn new(center: SensorPoint, size_fraction: f32) -> Result<Self, InvalidMeteringRegion> {
        if !(size_fraction > 0.0 && size_fraction <= 1.0) {
            return Err(InvalidMeteringRegion::SizeOutOfRange {
                fraction: size_fraction,
            });
        }
        Ok(Self { center, size_fraction })
    }

    pub fn center(self) -> SensorPoint {
        self.center
    }

    pub fn size_fraction(self) -> f32 {
        self.size_fraction
    }
}

/// Номер запроса «сфокусируйся сюда». Новый тап — новый номер, поэтому повтор того же
/// желаемого состояния не перезапускает фокус (движок уровневый: сравнивает номер).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct PointRequestId(pub u64);

/// Куда смотрит автоматика фокуса и экспозиции.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PointOfInterest {
    /// Непрерывный автофокус и замер по всему кадру (состояние по умолчанию).
    WholeFrame,
    /// Тап: однократный фокус и замер в регионе (iOS: жёлтый квадрат).
    Point {
        region: MeteringRegion,
        request: PointRequestId,
    },
}

/// Блокировка AE/AF (iOS: долгий тап, плашка «AE/AF LOCK»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AeAfLock {
    Unlocked,
    /// Фокус и экспозиция держатся при смене сцены, пока пользователь не снимет блокировку
    /// или не сменится камера (решение о сбросе — конфиг `camera-core`).
    Locked,
}

/// Состояние автофокуса из результата (`ACAMERA_CONTROL_AF_STATE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AfState {
    Inactive,
    PassiveScan,
    PassiveFocused,
    PassiveUnfocused,
    ActiveScan,
    FocusedLocked,
    NotFocusedLocked,
}

/// Состояние вендорского AE (`ACAMERA_CONTROL_AE_STATE`). Нужно только временному
/// вендорскому AE 08 и диагностике; наш AE (08a) его не читает.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AeState {
    Inactive,
    Searching,
    Converged,
    Locked,
    FlashRequired,
    Precapture,
}

/// Движется ли объектив (`ACAMERA_LENS_STATE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LensState {
    Stationary,
    Moving,
}

/// Мерцание сцены по детектору HAL (`ACAMERA_STATISTICS_SCENE_FLICKER`) — вход для 08a.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneFlicker {
    None,
    Mains50Hz,
    Mains60Hz,
}

/// Частота сети для защиты от мерцания ламп (решение 67: по умолчанию 50 Гц).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MainsFrequency {
    #[serde(rename = "50hz")]
    Hz50,
    #[serde(rename = "60hz")]
    Hz60,
}

/// Режим нашего AWB, который решает `camera-core` и исполняет AWB на потоке рендера.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum WhiteBalanceMode {
    /// Оценка по статистике RAW с плавной адаптацией.
    Auto,
    /// Держать последнюю точку белого (не переоценивать).
    Locked,
    /// Точка белого задана пользователем.
    Manual { setting: WhiteBalanceSetting },
}

/// Указание AWB от владельца 3A: режим и профиль. Поколение растёт с каждой публикацией,
/// чтобы исполнитель отличал новое указание от уже применённого.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct WhiteBalanceDirective {
    pub mode: WhiteBalanceMode,
    pub profile: ThreeAProfile,
}

impl WhiteBalanceDirective {
    /// Указание по умолчанию, пока движок камеры ничего не опубликовал.
    pub const DEFAULT: WhiteBalanceDirective = WhiteBalanceDirective {
        mode: WhiteBalanceMode::Auto,
        profile: ThreeAProfile::Photo,
    };
}

/// Номер публикации в [`DirectiveSlot`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct DirectiveGeneration(pub u64);

/// Нейтральный «почтовый ящик» между потоками (решение 64): писатель кладёт последнее
/// указание, читатель забирает самое свежее. Без очереди — промежуточные значения не нужны.
/// Замок держится только на копирование значения, никто никого не ждёт.
#[derive(Debug)]
pub struct DirectiveSlot<T> {
    shared: Arc<Mutex<SlotState<T>>>,
}

#[derive(Debug)]
struct SlotState<T> {
    generation: u64,
    value: Option<T>,
}

impl<T> Clone for DirectiveSlot<T> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<T: Copy> Default for DirectiveSlot<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy> DirectiveSlot<T> {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Mutex::new(SlotState {
                generation: 0,
                value: None,
            })),
        }
    }

    /// Положить новое указание; возвращает его поколение (первое — 1).
    pub fn publish(&self, value: T) -> DirectiveGeneration {
        // Отравленный замок здесь безопасен: состояние — пара копируемых полей, частично
        // записанным оно быть не может, поэтому продолжаем с ним, а не паникуем.
        let mut state = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        state.generation += 1;
        state.value = Some(value);
        DirectiveGeneration(state.generation)
    }

    /// Самое свежее указание с его поколением; `None`, пока ничего не публиковали.
    pub fn latest(&self) -> Option<(DirectiveGeneration, T)> {
        let state = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        state.value.map(|value| (DirectiveGeneration(state.generation), value))
    }

    /// Указание новее `seen` (для читателя, который уже применил `seen`).
    pub fn newer_than(&self, seen: Option<DirectiveGeneration>) -> Option<(DirectiveGeneration, T)> {
        self.latest().filter(|(generation, _)| Some(*generation) > seen)
    }
}

/// Поддержка компенсации экспозиции вендорским AE (`aeCompensationRange`/`Step`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "support")]
pub enum ExposureCompensationSupport {
    /// Диапазон `[0, 0]` — компенсации нет.
    NotSupported,
    /// Диапазон в шагах и размер шага в EV (OnePlus 11: −18…18 по 1/6).
    Supported {
        min_steps: i32,
        max_steps: i32,
        step_ev: Rational,
    },
}

/// Сколько регионов принимает каждая автоматика (`ACAMERA_CONTROL_MAX_REGIONS`: AE, AWB, AF).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct MaxRegions {
    pub auto_exposure: u32,
    pub auto_white_balance: u32,
    pub auto_focus: u32,
}

/// Режим автофокуса HAL (`ACAMERA_CONTROL_AF_MODE`), который камера объявила.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AfModeSupport {
    Off,
    Auto,
    Macro,
    ContinuousVideo,
    ContinuousPicture,
    ExtendedDepthOfField,
}

/// Режим антибандинга HAL (`aeAvailableAntibandingModes`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AntibandingModeSupport {
    Off,
    Hz50,
    Hz60,
    Auto,
}

/// Блокировка, объявленная камерой (`aeLockAvailable`, `awbLockAvailable`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LockSupport {
    Available,
    NotAvailable,
}

/// Возможности автоматики 3A камеры (из характеристик HAL).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ThreeACapabilities {
    pub exposure_compensation: ExposureCompensationSupport,
    pub max_regions: MaxRegions,
    pub af_modes: Vec<AfModeSupport>,
    pub ae_lock: LockSupport,
    pub antibanding_modes: Vec<AntibandingModeSupport>,
}

impl ThreeACapabilities {
    /// Камера без объявленной автоматики (честный ноль, а не выдумка).
    pub fn none() -> Self {
        Self {
            exposure_compensation: ExposureCompensationSupport::NotSupported,
            max_regions: MaxRegions::default(),
            af_modes: Vec::new(),
            ae_lock: LockSupport::NotAvailable,
            antibanding_modes: Vec::new(),
        }
    }

    pub fn supports_af_mode(&self, mode: AfModeSupport) -> bool {
        self.af_modes.contains(&mode)
    }
}

/// Номер серии с брекетингом.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct BurstId(pub u64);

/// Экспозиция одного кадра серии.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct BracketExposure {
    pub exposure_time: ExposureTime,
    pub iso: Iso,
}

/// Почему серия отвергнута до камеры.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum InvalidBracket {
    #[error("в серии нет кадров")]
    Empty,
    #[error("в серии {frames} кадров, предел {limit}")]
    TooManyFrames { frames: usize, limit: usize },
}

/// Намерение «N кадров с заданными экспозициями» (V10, `PER_FRAME_CONTROL`).
/// Каждая экспозиция приходит в результате ровно на свой кадр серии (проверяет smoke `3a`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExposureBracket {
    frames: Vec<BracketExposure>,
}

impl ExposureBracket {
    /// `limit` — предел длины серии из конфига движка (бюджет пула RAW-кадров).
    pub fn new(frames: Vec<BracketExposure>, limit: usize) -> Result<Self, InvalidBracket> {
        if frames.is_empty() {
            return Err(InvalidBracket::Empty);
        }
        if frames.len() > limit {
            return Err(InvalidBracket::TooManyFrames {
                frames: frames.len(),
                limit,
            });
        }
        Ok(Self { frames })
    }

    pub fn frames(&self) -> &[BracketExposure] {
        &self.frames
    }
}

/// Откуда пришёл результат: какой запрос его породил. Без этого кадры серии и кадр
/// триггера фокуса не отличить от повторяющегося потока.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum RequestOrigin {
    /// Повторяющийся запрос потока.
    Repeating,
    /// Разовый запрос с триггером автофокуса для тапа `request`.
    FocusTrigger { request: PointRequestId },
    /// Кадр `index` серии `burst`.
    Burst { burst: BurstId, index: u32 },
    /// Разовый кадр со вспышкой снимка `shot` (14d). Видоискатель его не показывает,
    /// кольцо снимков отдаёт его только по этой метке.
    FlashStill { shot: FlashShotId },
    /// HAL отдал результат, но запрос не опознан (контекст не прочитан) — со счётчиком.
    Unknown,
}

/// Состояние автоматики, которое HAL сообщил в результате одного кадра.
/// `None` в поле — HAL ключ не отдал (в RAW-only сессии AF_STATE нет вовсе, факт 08).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ThreeAResultState {
    pub af_state: Option<AfState>,
    pub ae_state: Option<AeState>,
    pub lens_state: Option<LensState>,
    /// Фактическая дистанция фокуса объектива на этом кадре, диоптрии.
    pub focus_distance_diopters: Option<f32>,
    pub scene_flicker: Option<SceneFlicker>,
    /// Что HAL сообщил о вспышке на этом кадре (14d); по нему кадр опознаётся как освещённый.
    pub flash: FlashResult,
    pub origin: RequestOrigin,
}

impl ThreeAResultState {
    /// Результат, в котором HAL не сообщил ничего об автоматике.
    pub fn unreported(origin: RequestOrigin) -> Self {
        Self {
            af_state: None,
            ae_state: None,
            lens_state: None,
            focus_distance_diopters: None,
            scene_flicker: None,
            flash: FlashResult::UNREPORTED,
            origin,
        }
    }
}

#[cfg(test)]
#[path = "three_a_tests.rs"]
mod tests;
