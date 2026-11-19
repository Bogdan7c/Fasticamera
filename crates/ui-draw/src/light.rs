//! Направление света для блика: покой и реакция на наклон телефона.
//!
//! Apple (219, 11:29): «в некоторых случаях освещение реагирует на движение устройства».
//! Какие элементы и как — не сказано, поэтому здесь `[estimate]`: свет «как будто неподвижен в мире»
//! и приходит сверху-слева; когда телефон наклоняют, блик на форме смещается так, будто свет остался
//! на месте. Числа — подгонка, не Apple.
//!
//! # Соглашение об углах
//! Угол `θ` отсчитывается по часовой стрелке от оси x экрана (ось y вниз); направление света —
//! `(cos θ, sin θ)` и означает «куда летит свет» (от источника к стеклу). Покой 45° — свет летит
//! вправо-вниз, то есть приходит сверху-слева. Блик использует `|dot(нормаль, свет)|`, поэтому знак
//! вектора на картинку не влияет, важна ось.
//!
//! # Наклон
//! Берётся крен телефона в плоскости экрана (вращение вокруг оси «из экрана»). Если верх телефона
//! уходит ВПРАВО (поворот по часовой стрелке на `φ`), свет в координатах экрана поворачивается
//! ПРОТИВ часовой: `θ = θ_покоя − φ` (свет остался на месте, ушёл экран). Тангаж (наклон «на себя» и
//! «от себя») игнорируется: он не определяет направление в плоскости. Крен ограничен
//! [`TiltLightConfig::max_tilt_deg`], телефон «лежит плашмя» → крена нет, свет возвращается в покой.

use serde::{Deserialize, Serialize};
use shell_contract::gravity::{GravityReading, GravitySample};
use thiserror::Error;

use crate::geometry::PxVector;

/// Единичное направление света в осях экрана (y вниз).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightDirection {
    x: f32,
    y: f32,
}

impl LightDirection {
    /// Из угла в градусах (по часовой стрелке от оси x, y вниз).
    pub fn from_angle_degrees(angle_deg: f32) -> Self {
        let (sin, cos) = angle_deg.to_radians().sin_cos();
        Self { x: cos, y: sin }
    }

    /// Угол в градусах, `(-180, 180]`.
    pub fn angle_degrees(&self) -> f32 {
        self.y.atan2(self.x).to_degrees()
    }

    /// Единичный вектор направления.
    pub fn as_vector(&self) -> PxVector {
        PxVector::new(self.x, self.y)
    }
}

/// Настройки реакции света на наклон.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TiltLightConfig {
    /// Предел отклонения света от покоя, градусы. `[estimate]`: подгонка, не Apple.
    pub max_tilt_deg: f32,
    /// Постоянная времени сглаживания, секунды: за неё свет проходит ~63 % пути к цели.
    /// `[estimate]`: подгонка, не Apple.
    pub smoothing_time_constant_s: f32,
    /// Телефон «лежит плашмя», если доля гравитации в плоскости экрана меньше этой
    /// (`sin` угла от горизонтали); крен тогда не определён. `[estimate]`.
    pub flat_threshold: f32,
}

impl Default for TiltLightConfig {
    fn default() -> Self {
        Self {
            max_tilt_deg: 60.0,
            smoothing_time_constant_s: 0.12,
            flat_threshold: 0.2,
        }
    }
}

/// Настройки света заданы неверно.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum LightError {
    #[error("max_tilt_deg должен быть в (0, 180], получено {0}")]
    InvalidMaxTilt(f32),
    #[error("постоянная сглаживания должна быть конечной и > 0, получено {0}")]
    InvalidTimeConstant(f32),
    #[error("flat_threshold должен быть в [0, 1), получено {0}")]
    InvalidFlatThreshold(f32),
}

impl TiltLightConfig {
    pub fn validate(&self) -> Result<(), LightError> {
        if !(self.max_tilt_deg.is_finite() && self.max_tilt_deg > 0.0 && self.max_tilt_deg <= 180.0) {
            return Err(LightError::InvalidMaxTilt(self.max_tilt_deg));
        }
        if !(self.smoothing_time_constant_s.is_finite() && self.smoothing_time_constant_s > 0.0) {
            return Err(LightError::InvalidTimeConstant(self.smoothing_time_constant_s));
        }
        if !(self.flat_threshold.is_finite() && (0.0..1.0).contains(&self.flat_threshold)) {
            return Err(LightError::InvalidFlatThreshold(self.flat_threshold));
        }
        Ok(())
    }
}

/// Что сделало очередное чтение гравитации.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TiltUpdate {
    /// Датчик ещё ничего не прислал; свет в покое.
    NoSampleYet,
    /// Показание учтено, свет сдвинут к цели с экспоненциальным сглаживанием.
    Smoothed,
    /// То же показание, что уже учтено (опрос чаще датчика): ничего не изменилось.
    DuplicateSample,
    /// Время показания меньше уже учтённого: сглаживание не применено, показание только запомнено
    /// как новая точка отсчёта времени, счётчик [`TiltLight::time_regressions`] вырос.
    TimeWentBackwards,
}

/// Свет, следящий за наклоном. Сглаживает ПО ВРЕМЕНИ ДАТЧИКА (BOOTTIME), а не «на кадр»: при 60 и
/// 120 Гц показаний результат одинаков при равном времени.
#[derive(Debug, Clone)]
pub struct TiltLight {
    config: TiltLightConfig,
    rest_angle_deg: f32,
    /// Сглаженное отклонение от покоя, градусы.
    offset_deg: f32,
    last_sample_nanos: Option<i64>,
    time_regressions: u64,
}

impl TiltLight {
    /// Свет в покое под углом `rest` (из `SpecularStyle::rest_angle_deg`).
    pub fn new(config: TiltLightConfig, rest: LightDirection) -> Result<Self, LightError> {
        config.validate()?;
        Ok(Self {
            config,
            rest_angle_deg: rest.angle_degrees(),
            offset_deg: 0.0,
            last_sample_nanos: None,
            time_regressions: 0,
        })
    }

    /// Текущее направление света.
    pub fn direction(&self) -> LightDirection {
        LightDirection::from_angle_degrees(self.rest_angle_deg - self.offset_deg)
    }

    /// Сколько раз время показаний шло назад.
    pub fn time_regressions(&self) -> u64 {
        self.time_regressions
    }

    /// Учесть то, что лежит в ящике гравитации.
    pub fn apply_reading(&mut self, reading: GravityReading) -> TiltUpdate {
        match reading {
            GravityReading::NoSampleYet => TiltUpdate::NoSampleYet,
            GravityReading::Latest { sample, .. } => self.apply_sample(sample),
        }
    }

    /// Учесть одно показание.
    pub fn apply_sample(&mut self, sample: GravitySample) -> TiltUpdate {
        let now = sample.time.as_nanos();
        let target_deg = self.target_offset_deg(&sample);
        match self.last_sample_nanos {
            None => {
                // Первое показание: времени «до него» нет, сглаживать не от чего — берём цель сразу.
                self.last_sample_nanos = Some(now);
                self.offset_deg = target_deg;
                TiltUpdate::Smoothed
            }
            Some(last) if now == last => TiltUpdate::DuplicateSample,
            Some(last) if now < last => {
                self.time_regressions = self.time_regressions.saturating_add(1);
                self.last_sample_nanos = Some(now);
                TiltUpdate::TimeWentBackwards
            }
            Some(last) => {
                let dt_seconds = (now - last) as f32 * 1.0e-9;
                // Точное решение экспоненциального сглаживания за интервал dt: от частоты опроса не зависит.
                let blend = 1.0 - (-dt_seconds / self.config.smoothing_time_constant_s).exp();
                self.offset_deg += (target_deg - self.offset_deg) * blend;
                self.last_sample_nanos = Some(now);
                TiltUpdate::Smoothed
            }
        }
    }

    /// Целевое отклонение света (градусы) для показания: крен φ в пределах ±max, плашмя — 0.
    fn target_offset_deg(&self, sample: &GravitySample) -> f32 {
        let vector = sample.vector;
        let in_plane = vector.x.hypot(vector.y);
        let total = (in_plane * in_plane + vector.z * vector.z).sqrt();
        if !(total.is_finite() && total > f32::EPSILON) || in_plane / total < self.config.flat_threshold {
            return 0.0;
        }
        // «Вверх мира» в осях телефона = (gx, gy); верх телефона вправо (по часовой на φ) даёт gx = −sin φ.
        let roll_deg = (-vector.x).atan2(vector.y).to_degrees();
        roll_deg.clamp(-self.config.max_tilt_deg, self.config.max_tilt_deg)
    }
}
