//! Физическая ориентация по акселерометру (решение владельца, сессия 02: свой классификатор).
//!
//! Акселерометр в покое показывает «реакцию опоры»: вектор, направленный вверх, длиной ~g.
//! Оси устройства Android: x — вправо по экрану, y — вверх по экрану, z — из экрана.
//! * Угол в плоскости экрана `atan2(-x, y)`: 0° — портрет, −90° — верх смотрит влево,
//!   +90° — вправо, ±180° — вверх ногами.
//! * Наклон к плоскости экрана `asin(|z| / |a|)`: 0° — телефон вертикально, 90° — лежит
//!   плашмя. Когда телефон лежит, угол в плоскости шумит, поэтому ориентацию не меняем.
//!
//! Защита от дрожания:
//! * гистерезис: в новую ориентацию переходим, только когда угол зашёл в её сектор глубже
//!   на `hysteresis_degrees` (у самой границы 45° ориентация не скачет);
//! * выдержка: новая ориентация должна продержаться `settle_ms` по времени датчика;
//! * встряска: показания, чья длина далеко от g, пропускаются.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use shell_contract::clock::BoottimeNanos;
use shell_contract::orientation::DeviceOrientation;

/// Стандартное ускорение свободного падения, м/с².
const STANDARD_GRAVITY: f32 = 9.806_65;

/// Половина сектора одной ориентации (четыре сектора по 90°).
const HALF_SECTOR_DEGREES: f32 = 45.0;

/// Пороги классификатора.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct OrientationClassifierConfig {
    /// Наклон к плоскости экрана, выше которого телефон считается лежащим (ориентация не меняется).
    pub max_tilt_degrees: f32,
    /// Насколько глубже границы сектора должен зайти угол, чтобы сменить ориентацию.
    pub hysteresis_degrees: f32,
    /// Сколько новая ориентация должна продержаться, миллисекунды по времени датчика.
    pub settle_ms: u64,
    /// Допустимая длина вектора относительно g: вне `[min, max]` — встряска, показание пропускается.
    pub min_gravity_ratio: f32,
    pub max_gravity_ratio: f32,
}

impl Default for OrientationClassifierConfig {
    /// Значения близки к системному `WindowOrientationListener` AOSP (допуск наклона, выдержка).
    fn default() -> Self {
        Self {
            max_tilt_degrees: 60.0,
            hysteresis_degrees: 10.0,
            settle_ms: 250,
            min_gravity_ratio: 0.6,
            max_gravity_ratio: 1.4,
        }
    }
}

/// Неверные пороги классификатора.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum OrientationConfigError {
    #[error("max_tilt_degrees должен быть в (0, 90], а не {0}")]
    InvalidTilt(f32),
    #[error("hysteresis_degrees должен быть в [0, 45), а не {0}")]
    InvalidHysteresis(f32),
    #[error("диапазон длины вектора должен быть 0 < min ({min}) < 1 < max ({max})")]
    InvalidGravityRange { min: f32, max: f32 },
}

impl OrientationClassifierConfig {
    pub fn validate(self) -> Result<Self, OrientationConfigError> {
        if !(self.max_tilt_degrees > 0.0 && self.max_tilt_degrees <= 90.0) {
            return Err(OrientationConfigError::InvalidTilt(self.max_tilt_degrees));
        }
        if !(self.hysteresis_degrees >= 0.0 && self.hysteresis_degrees < HALF_SECTOR_DEGREES) {
            return Err(OrientationConfigError::InvalidHysteresis(self.hysteresis_degrees));
        }
        let (min, max) = (self.min_gravity_ratio, self.max_gravity_ratio);
        if !(min > 0.0 && min < 1.0 && max > 1.0 && max.is_finite()) {
            return Err(OrientationConfigError::InvalidGravityRange { min, max });
        }
        Ok(self)
    }
}

/// Одно показание акселерометра, м/с², в осях устройства.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AccelerationSample {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub time: BoottimeNanos,
}

/// Классификатор: показания акселерометра → смена ориентации.
#[derive(Debug, Clone)]
pub struct OrientationClassifier {
    config: OrientationClassifierConfig,
    current: Option<DeviceOrientation>,
    /// Кандидат на смену и момент, с которого он держится.
    pending: Option<(DeviceOrientation, BoottimeNanos)>,
}

impl OrientationClassifier {
    pub fn new(config: OrientationClassifierConfig) -> Result<Self, OrientationConfigError> {
        Ok(Self {
            config: config.validate()?,
            current: None,
            pending: None,
        })
    }

    /// Текущая ориентация (`None`, пока не было ни одного устойчивого показания).
    pub fn current(&self) -> Option<DeviceOrientation> {
        self.current
    }

    /// Принимает показание; возвращает новую ориентацию, только если она сменилась.
    pub fn observe(&mut self, sample: AccelerationSample) -> Option<DeviceOrientation> {
        let Some(target) = self.target_orientation(sample) else {
            // Лежит плашмя или трясётся: незавершённую смену отменяем.
            self.pending = None;
            return None;
        };
        if Some(target) == self.current {
            self.pending = None;
            return None;
        }
        match self.pending {
            Some((candidate, since)) if candidate == target => {
                let settle = Duration::from_millis(self.config.settle_ms);
                if sample.time.saturating_duration_since(since) >= settle {
                    self.current = Some(target);
                    self.pending = None;
                    return Some(target);
                }
                None
            }
            _ => {
                self.pending = Some((target, sample.time));
                None
            }
        }
    }

    /// Ориентация, к которой тянет это показание с учётом гистерезиса; `None` — показание не годится.
    fn target_orientation(&self, sample: AccelerationSample) -> Option<DeviceOrientation> {
        let length = (sample.x * sample.x + sample.y * sample.y + sample.z * sample.z).sqrt();
        let gravity_ratio = length / STANDARD_GRAVITY;
        if !(gravity_ratio >= self.config.min_gravity_ratio && gravity_ratio <= self.config.max_gravity_ratio) {
            return None;
        }
        let tilt_degrees = (sample.z.abs() / length).asin().to_degrees();
        if tilt_degrees > self.config.max_tilt_degrees {
            return None;
        }
        let screen_angle = (-sample.x).atan2(sample.y).to_degrees();
        let nearest = nearest_orientation(screen_angle);
        match self.current {
            // Для смены угол должен зайти в сектор новой ориентации глубже гистерезиса.
            Some(current) if nearest != current => {
                let depth_needed = HALF_SECTOR_DEGREES - self.config.hysteresis_degrees;
                if angular_distance(screen_angle, center_degrees(nearest)) <= depth_needed {
                    Some(nearest)
                } else {
                    Some(current)
                }
            }
            _ => Some(nearest),
        }
    }
}

/// Центр сектора ориентации на окружности угла в плоскости экрана.
fn center_degrees(orientation: DeviceOrientation) -> f32 {
    match orientation {
        DeviceOrientation::Portrait => 0.0,
        DeviceOrientation::LandscapeLeft => -90.0,
        DeviceOrientation::LandscapeRight => 90.0,
        DeviceOrientation::PortraitUpsideDown => 180.0,
    }
}

/// Кратчайшее расстояние между углами на окружности, 0–180°.
fn angular_distance(first_degrees: f32, second_degrees: f32) -> f32 {
    let difference = (first_degrees - second_degrees).rem_euclid(360.0);
    difference.min(360.0 - difference)
}

fn nearest_orientation(screen_angle_degrees: f32) -> DeviceOrientation {
    [
        DeviceOrientation::Portrait,
        DeviceOrientation::LandscapeLeft,
        DeviceOrientation::LandscapeRight,
        DeviceOrientation::PortraitUpsideDown,
    ]
    .into_iter()
    .min_by(|first, second| {
        angular_distance(screen_angle_degrees, center_degrees(*first))
            .total_cmp(&angular_distance(screen_angle_degrees, center_degrees(*second)))
    })
    .unwrap_or(DeviceOrientation::Portrait)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: i64 = 1_000_000;

    /// Показание вертикально стоящего телефона, повёрнутого на `angle` в плоскости экрана
    /// (0° — портрет, −90° — верх влево), с наклоном `tilt` к плоскости экрана.
    fn upright(angle_degrees: f32, tilt_degrees: f32, time_ms: i64) -> AccelerationSample {
        let in_plane = STANDARD_GRAVITY * tilt_degrees.to_radians().cos();
        let angle = angle_degrees.to_radians();
        AccelerationSample {
            x: -in_plane * angle.sin(),
            y: in_plane * angle.cos(),
            z: STANDARD_GRAVITY * tilt_degrees.to_radians().sin(),
            time: BoottimeNanos::from_nanos(time_ms * MS),
        }
    }

    fn classifier() -> OrientationClassifier {
        OrientationClassifier::new(OrientationClassifierConfig::default()).expect("классификатор")
    }

    /// Подаёт одинаковые показания каждые 20 мс (50 Гц) и возвращает все смены ориентации.
    fn feed(
        classifier: &mut OrientationClassifier,
        angle: f32,
        tilt: f32,
        from_ms: i64,
        to_ms: i64,
    ) -> Vec<DeviceOrientation> {
        (from_ms..=to_ms)
            .step_by(20)
            .filter_map(|time_ms| classifier.observe(upright(angle, tilt, time_ms)))
            .collect()
    }

    #[test]
    fn upright_phone_settles_to_portrait_after_settle_time() {
        let mut classifier = classifier();
        assert_eq!(feed(&mut classifier, 0.0, 10.0, 0, 200), Vec::new(), "до выдержки рано");
        assert_eq!(
            feed(&mut classifier, 0.0, 10.0, 220, 300),
            vec![DeviceOrientation::Portrait]
        );
        assert_eq!(classifier.current(), Some(DeviceOrientation::Portrait));
    }

    #[test]
    fn each_quarter_turn_maps_to_its_orientation() {
        for (angle, expected) in [
            (-90.0, DeviceOrientation::LandscapeLeft),
            (90.0, DeviceOrientation::LandscapeRight),
            (180.0, DeviceOrientation::PortraitUpsideDown),
            (0.0, DeviceOrientation::Portrait),
        ] {
            let mut classifier = classifier();
            assert_eq!(
                feed(&mut classifier, angle, 0.0, 0, 400),
                vec![expected],
                "угол {angle}"
            );
        }
    }

    #[test]
    fn rotation_switches_once_and_reports_change() {
        let mut classifier = classifier();
        feed(&mut classifier, 0.0, 0.0, 0, 400);
        let changes = feed(&mut classifier, -90.0, 0.0, 420, 1000);
        assert_eq!(changes, vec![DeviceOrientation::LandscapeLeft]);
    }

    #[test]
    fn hysteresis_holds_orientation_near_sector_border() {
        let mut classifier = classifier();
        feed(&mut classifier, 0.0, 0.0, 0, 400);
        // 50° от портрета: ближе к landscape_right (40°), но не глубже гистерезиса (35°).
        assert_eq!(feed(&mut classifier, 50.0, 0.0, 420, 2000), Vec::new());
        assert_eq!(classifier.current(), Some(DeviceOrientation::Portrait));
        // 56°: до центра landscape_right 34° ≤ 35° — смена.
        assert_eq!(
            feed(&mut classifier, 56.0, 0.0, 2020, 3000),
            vec![DeviceOrientation::LandscapeRight]
        );
    }

    #[test]
    fn short_excursion_shorter_than_settle_is_ignored() {
        let mut classifier = classifier();
        feed(&mut classifier, 0.0, 0.0, 0, 400);
        assert_eq!(feed(&mut classifier, 90.0, 0.0, 420, 560), Vec::new());
        assert_eq!(feed(&mut classifier, 0.0, 0.0, 580, 1200), Vec::new());
        assert_eq!(classifier.current(), Some(DeviceOrientation::Portrait));
    }

    #[test]
    fn flat_phone_does_not_change_orientation() {
        let mut classifier = classifier();
        feed(&mut classifier, 0.0, 0.0, 0, 400);
        // Лежит почти плашмя (наклон 80°), «угол» указывает в landscape — игнорируем.
        assert_eq!(feed(&mut classifier, 90.0, 80.0, 420, 3000), Vec::new());
        assert_eq!(classifier.current(), Some(DeviceOrientation::Portrait));
    }

    #[test]
    fn shake_samples_are_skipped() {
        let mut classifier = classifier();
        feed(&mut classifier, 0.0, 0.0, 0, 400);
        let mut shaken = upright(90.0, 0.0, 500);
        shaken.x *= 3.0;
        for offset in 0..30 {
            shaken.time = BoottimeNanos::from_nanos((500 + offset * 20) * MS);
            assert_eq!(classifier.observe(shaken), None);
        }
        assert_eq!(classifier.current(), Some(DeviceOrientation::Portrait));
    }

    #[test]
    fn invalid_config_is_rejected() {
        let tilt = OrientationClassifierConfig {
            max_tilt_degrees: 0.0,
            ..OrientationClassifierConfig::default()
        };
        assert!(matches!(
            OrientationClassifier::new(tilt),
            Err(OrientationConfigError::InvalidTilt(_))
        ));
        let hysteresis = OrientationClassifierConfig {
            hysteresis_degrees: 45.0,
            ..OrientationClassifierConfig::default()
        };
        assert!(OrientationClassifier::new(hysteresis).is_err());
        let gravity = OrientationClassifierConfig {
            min_gravity_ratio: 1.2,
            ..OrientationClassifierConfig::default()
        };
        assert!(OrientationClassifier::new(gravity).is_err());
    }

    #[test]
    fn angular_distance_wraps_around() {
        assert_eq!(angular_distance(170.0, -170.0), 20.0);
        assert_eq!(angular_distance(-90.0, 90.0), 180.0);
    }
}
