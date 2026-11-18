//! Намерения управления камерой: шаблон запроса и домен ручных параметров (сессия 08).
//!
//! Каждый ручной параметр — [`Control`]: либо его ведёт автомат (`Auto`), либо пользователь
//! (`Manual(значение)`). Кто именно «автомат» для параметра, решает `camera-core`:
//! фокус — вендорский AF через HAL, экспозиция — наш AE (решение 63; в 08 временно вендорский,
//! решение 66), баланс белого — наш AWB (решение 64). Значения — newtype-ы с проверкой
//! на входе: невозможное значение (NaN, ноль, отрицательное) не доходит до планирования запроса.
//! Диапазоны конкретной камеры здесь не проверяются — это делает `camera-core` по
//! [`crate::capabilities::CameraCapabilities`] и сообщает явный итог «ограничено до …».

use std::time::Duration;

use serde::Serialize;
use thiserror::Error;

/// Для чего поток: определяет шаблон запроса Camera2 (`ACameraDevice_request_template`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureIntent {
    /// Видоискатель (`TEMPLATE_PREVIEW`).
    Preview,
    /// Запись видео (`TEMPLATE_RECORD`): HAL настраивает 3A на плавность.
    VideoRecord,
}

/// Кто ведёт параметр: автомат или пользователь.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case", tag = "mode", content = "value")]
pub enum Control<T> {
    Auto,
    Manual(T),
}

impl<T: Copy> Control<T> {
    /// Ручное значение, если параметр ручной.
    pub fn manual_value(&self) -> Option<T> {
        match self {
            Control::Auto => None,
            Control::Manual(value) => Some(*value),
        }
    }

    pub fn is_auto(&self) -> bool {
        matches!(self, Control::Auto)
    }
}

/// Почему значение ручного параметра отвергнуто ещё до камеры.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum InvalidControlValue {
    #[error("ISO должно быть больше нуля")]
    ZeroIso,
    #[error("выдержка должна быть больше нуля")]
    ZeroExposureTime,
    #[error("дистанция фокуса {diopters} дптр: нужна конечная и неотрицательная")]
    InvalidFocusDistance { diopters: f32 },
    #[error("компенсация экспозиции {ev} EV не конечна")]
    NonFiniteExposureBias { ev: f32 },
    #[error("цветовая температура {kelvin} K: нужна конечная и положительная")]
    InvalidColorTemperature { kelvin: f64 },
    #[error("оттенок Duv {duv} не конечен")]
    NonFiniteTint { duv: f64 },
}

/// Светочувствительность сенсора (`ACAMERA_SENSOR_SENSITIVITY`, ISO 12232).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct Iso(u32);

impl Iso {
    pub fn new(value: u32) -> Result<Self, InvalidControlValue> {
        if value == 0 {
            return Err(InvalidControlValue::ZeroIso);
        }
        Ok(Self(value))
    }

    pub fn value(self) -> u32 {
        self.0
    }
}

/// Выдержка (`ACAMERA_SENSOR_EXPOSURE_TIME`). HAL квантует её по строке сенсора:
/// на OnePlus 11 1 мс → 0,996 мс (зонд 08), поэтому результат сравнивают с допуском.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ExposureTime(Duration);

impl ExposureTime {
    pub fn new(duration: Duration) -> Result<Self, InvalidControlValue> {
        if duration.is_zero() {
            return Err(InvalidControlValue::ZeroExposureTime);
        }
        Ok(Self(duration))
    }

    pub fn duration(self) -> Duration {
        self.0
    }
}

/// Дистанция фокуса в диоптриях (`ACAMERA_LENS_FOCUS_DISTANCE`): 0 — бесконечность,
/// больше — ближе. Калибровка объектива на OnePlus 11 `APPROXIMATE` (00), то есть числа
/// не метры с точностью, а монотонная шкала.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct FocusDistance(f32);

impl FocusDistance {
    pub const INFINITY: FocusDistance = FocusDistance(0.0);

    pub fn from_diopters(diopters: f32) -> Result<Self, InvalidControlValue> {
        if !diopters.is_finite() || diopters < 0.0 {
            return Err(InvalidControlValue::InvalidFocusDistance { diopters });
        }
        Ok(Self(diopters))
    }

    pub fn diopters(self) -> f32 {
        self.0
    }
}

/// Компенсация экспозиции в ступенях EV (+1 — вдвое светлее).
///
/// Хранится непрерывной: наш AE (08a) применяет её как множитель цели. Вендорский AE (08)
/// понимает только шаги `aeCompensationStep` (1/6 EV на OnePlus 11) — квантование и обрезка
/// по диапазону делаются при планировании запроса с явным итогом.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ExposureBias(f32);

impl ExposureBias {
    pub const NONE: ExposureBias = ExposureBias(0.0);

    pub fn from_ev(ev: f32) -> Result<Self, InvalidControlValue> {
        if !ev.is_finite() {
            return Err(InvalidControlValue::NonFiniteExposureBias { ev });
        }
        Ok(Self(ev))
    }

    pub fn ev(self) -> f32 {
        self.0
    }
}

/// Коррелированная цветовая температура источника света, кельвины.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ColorTemperature(f64);

impl ColorTemperature {
    pub fn from_kelvin(kelvin: f64) -> Result<Self, InvalidControlValue> {
        if !kelvin.is_finite() || kelvin <= 0.0 {
            return Err(InvalidControlValue::InvalidColorTemperature { kelvin });
        }
        Ok(Self(kelvin))
    }

    pub fn kelvin(self) -> f64 {
        self.0
    }
}

/// Оттенок: расстояние от локуса Планка в CIE 1960 uv (Duv), знак по CIE —
/// плюс выше локуса, к зелёному. Шкала UI (как «Tint» у Apple/Adobe) — забота экрана (15).
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Tint(f64);

impl Tint {
    pub const NEUTRAL: Tint = Tint(0.0);

    pub fn from_duv(duv: f64) -> Result<Self, InvalidControlValue> {
        if !duv.is_finite() {
            return Err(InvalidControlValue::NonFiniteTint { duv });
        }
        Ok(Self(duv))
    }

    pub fn duv(self) -> f64 {
        self.0
    }
}

/// Ручной баланс белого: точка белого сцены как «температура + оттенок».
/// В нейтраль конкретной камеры её переводит AWB по DNG-профилю своего потока (решение 64).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct WhiteBalanceSetting {
    pub temperature: ColorTemperature,
    pub tint: Tint,
}

/// Домен ручных параметров: что пользователь закрепил, а что ведёт автомат.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ManualControl {
    pub iso: Control<Iso>,
    pub shutter: Control<ExposureTime>,
    pub focus: Control<FocusDistance>,
    pub white_balance: Control<WhiteBalanceSetting>,
    /// Компенсация экспозиции. Действует, пока хотя бы один из `iso`/`shutter` на автомате;
    /// при полностью ручной экспозиции её игнорируют с явной пометкой (как в iOS — сверить с 09).
    pub exposure_bias: ExposureBias,
}

impl ManualControl {
    /// Всё на автомате — значение по умолчанию для нового потока.
    pub const AUTOMATIC: ManualControl = ManualControl {
        iso: Control::Auto,
        shutter: Control::Auto,
        focus: Control::Auto,
        white_balance: Control::Auto,
        exposure_bias: ExposureBias::NONE,
    };

    /// Экспозиция полностью ручная (обе величины закреплены).
    pub fn exposure_fully_manual(&self) -> bool {
        !self.iso.is_auto() && !self.shutter.is_auto()
    }
}

impl Default for ManualControl {
    fn default() -> Self {
        Self::AUTOMATIC
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impossible_values_are_rejected_before_the_camera() {
        assert_eq!(Iso::new(0), Err(InvalidControlValue::ZeroIso));
        assert_eq!(
            ExposureTime::new(Duration::ZERO),
            Err(InvalidControlValue::ZeroExposureTime)
        );
        assert!(FocusDistance::from_diopters(-0.1).is_err());
        assert!(FocusDistance::from_diopters(f32::NAN).is_err());
        assert!(ExposureBias::from_ev(f32::INFINITY).is_err());
        assert!(ColorTemperature::from_kelvin(0.0).is_err());
        assert!(Tint::from_duv(f64::NAN).is_err());
        assert_eq!(Iso::new(400).map(Iso::value), Ok(400));
    }

    #[test]
    fn fully_manual_exposure_needs_both_iso_and_shutter() {
        let mut controls = ManualControl::AUTOMATIC;
        assert!(!controls.exposure_fully_manual());
        controls.iso = Control::Manual(Iso(800));
        assert!(!controls.exposure_fully_manual(), "полуавтомат — не полностью ручная");
        controls.shutter = Control::Manual(ExposureTime(Duration::from_millis(1)));
        assert!(controls.exposure_fully_manual());
        assert_eq!(controls.iso.manual_value(), Some(Iso(800)));
    }
}
