//! Модель пружины и перевод между параметрами Apple (`motion.md` 1.2, 1.5).

use std::f64::consts::PI;

use thiserror::Error;

use crate::time::MotionPreference;

/// Ошибка построения пружины или анимации. Некорректный вход не «чинится молча».
#[derive(Debug, Clone, PartialEq, Error)]
pub enum SpringError {
    #[error("duration должна быть конечной и > 0, получено {0}")]
    InvalidDuration(f64),
    #[error("bounce должен лежать в (-1, 1], получено {0} (bounce = -1 — бесконечное затухание: пружина не движется)")]
    BounceOutOfRange(f64),
    #[error("response должен быть конечным и >= 0, получено {0}")]
    InvalidResponse(f64),
    #[error("dampingFraction должна быть конечной и >= 0, получено {0}")]
    InvalidDampingFraction(f64),
    #[error("масса должна быть конечной и > 0, получено {0}")]
    InvalidMass(f64),
    #[error("жёсткость должна быть конечной и > 0, получено {0}")]
    InvalidStiffness(f64),
    #[error("затухание должно быть конечным и >= 0, получено {0}")]
    InvalidDamping(f64),
    #[error("критерий покоя: {field} должен быть конечным и {requirement}, получено {value}")]
    InvalidRestCriterion {
        field: &'static str,
        requirement: &'static str,
        value: f64,
    },
    #[error("входное значение анимации не конечно: {field} = {value}")]
    NonFiniteInput { field: &'static str, value: f64 },
    #[error("лимит живых вкладов должен быть >= 1")]
    InvalidContributionLimit,
}

/// Физические параметры пружины: `m·x'' + c·x' + k·x = 0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhysicalSpring {
    pub mass: f64,
    pub stiffness: f64,
    pub damping: f64,
}

/// Воспринимаемые параметры SwiftUI (`Spring(duration:bounce:)`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PerceptualSpring {
    pub duration_s: f64,
    pub bounce: f64,
}

/// Вид пружины внутри: обычная физическая или «бесконечно жёсткая».
#[derive(Debug, Clone, Copy, PartialEq)]
enum SpringKind {
    Physical(PhysicalSpring),
    Immediate,
}

/// Пружина iOS. Неизменяемое значение; все конструкторы проверяют вход.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    kind: SpringKind,
}

// Источники пресетов: `motion.md` 1.5, `[doc]` D `swiftui/spring/{smooth,snappy,bouncy}(duration:extraBounce:)`.
/// Длительность стандартных пружин Apple по умолчанию.
pub const PRESET_DURATION_S: f64 = 0.5;
/// Базовый bounce `.smooth`.
pub const SMOOTH_BOUNCE: f64 = 0.0;
/// Базовый bounce `.snappy`.
pub const SNAPPY_BOUNCE: f64 = 0.15;
/// Базовый bounce `.bouncy`.
pub const BOUNCY_BOUNCE: f64 = 0.3;
/// `Animation.default` (iOS 17+): response (`[doc]` D `swiftui/animation/default`).
pub const DEFAULT_ANIMATION_RESPONSE_S: f64 = 0.55;
/// `Animation.default`: dampingFraction.
pub const DEFAULT_ANIMATION_DAMPING_FRACTION: f64 = 1.0;
/// `interactiveSpring`: response (`[doc]` D `swiftui/animation/interactivespring(...)`).
pub const INTERACTIVE_SPRING_RESPONSE_S: f64 = 0.15;
/// `interactiveSpring`: dampingFraction.
pub const INTERACTIVE_SPRING_DAMPING_FRACTION: f64 = 0.86;

impl Spring {
    /// `Spring(duration:bounce:)`. Формулы — `motion.md` 1.2 (при массе 1):
    /// `k = (2π/duration)²`, `ζ = 1 − bounce` при `bounce ≥ 0`, `ζ = 1/(1+bounce)` при `bounce < 0`,
    /// `c = 4π·ζ/duration`.
    ///
    /// Решение по границе: `bounce = -1` (ζ → ∞, пружина застыла) — **ошибка**
    /// [`SpringError::BounceOutOfRange`], а не подмена близким числом. `bounce = 1` допустим
    /// (затухания нет, пружина не успокаивается никогда).
    pub fn from_duration_bounce(duration_s: f64, bounce: f64) -> Result<Self, SpringError> {
        if !duration_s.is_finite() || duration_s <= 0.0 {
            return Err(SpringError::InvalidDuration(duration_s));
        }
        // Нижняя граница исключена, верхняя включена; NaN отсекается `is_finite`.
        if !bounce.is_finite() || bounce <= -1.0 || bounce > 1.0 {
            return Err(SpringError::BounceOutOfRange(bounce));
        }
        Ok(Self::from_response_unchecked(
            duration_s,
            damping_ratio_from_bounce(bounce),
        ))
    }

    /// `Spring(response:dampingRatio:)`. `response = 0` — по определению Apple «бесконечно жёсткая
    /// пружина для интерактивных анимаций» (`motion.md` 1.5), то есть [`Spring::immediate`].
    pub fn from_response_damping_fraction(response_s: f64, damping_fraction: f64) -> Result<Self, SpringError> {
        if !response_s.is_finite() || response_s < 0.0 {
            return Err(SpringError::InvalidResponse(response_s));
        }
        if !damping_fraction.is_finite() || damping_fraction < 0.0 {
            return Err(SpringError::InvalidDampingFraction(damping_fraction));
        }
        if response_s == 0.0 {
            return Ok(Self::immediate());
        }
        Ok(Self::from_response_unchecked(response_s, damping_fraction))
    }

    /// Прямые физические параметры (`UISpringTimingParameters(mass:stiffness:damping:)`).
    pub fn from_physical(mass: f64, stiffness: f64, damping: f64) -> Result<Self, SpringError> {
        if !mass.is_finite() || mass <= 0.0 {
            return Err(SpringError::InvalidMass(mass));
        }
        if !stiffness.is_finite() || stiffness <= 0.0 {
            return Err(SpringError::InvalidStiffness(stiffness));
        }
        if !damping.is_finite() || damping < 0.0 {
            return Err(SpringError::InvalidDamping(damping));
        }
        Ok(Self {
            kind: SpringKind::Physical(PhysicalSpring {
                mass,
                stiffness,
                damping,
            }),
        })
    }

    /// «Бесконечно жёсткая» пружина (`response = 0`): значение идёт за целью 1:1, скорости нет.
    pub const fn immediate() -> Self {
        Self {
            kind: SpringKind::Immediate,
        }
    }

    /// `.smooth` — duration 0,5, bounce 0 (k 157,91, c 25,13).
    pub fn smooth() -> Self {
        Self::from_duration_unchecked(PRESET_DURATION_S, SMOOTH_BOUNCE)
    }

    /// `.snappy` — duration 0,5, bounce 0,15 (k 157,91, c 21,36).
    pub fn snappy() -> Self {
        Self::from_duration_unchecked(PRESET_DURATION_S, SNAPPY_BOUNCE)
    }

    /// `.bouncy` — duration 0,5, bounce 0,3 (k 157,91, c 17,59).
    pub fn bouncy() -> Self {
        Self::from_duration_unchecked(PRESET_DURATION_S, BOUNCY_BOUNCE)
    }

    /// `.smooth(duration:)` с своей длительностью.
    pub fn smooth_with_duration(duration_s: f64) -> Result<Self, SpringError> {
        Self::from_duration_bounce(duration_s, SMOOTH_BOUNCE)
    }

    /// `.snappy(duration:)` с своей длительностью.
    pub fn snappy_with_duration(duration_s: f64) -> Result<Self, SpringError> {
        Self::from_duration_bounce(duration_s, SNAPPY_BOUNCE)
    }

    /// `.bouncy(duration:)` с своей длительностью.
    pub fn bouncy_with_duration(duration_s: f64) -> Result<Self, SpringError> {
        Self::from_duration_bounce(duration_s, BOUNCY_BOUNCE)
    }

    /// `Animation.default` (iOS 17+): response 0,55, dampingFraction 1.
    pub fn animation_default() -> Self {
        Self::from_response_unchecked(DEFAULT_ANIMATION_RESPONSE_S, DEFAULT_ANIMATION_DAMPING_FRACTION)
    }

    /// `interactiveSpring`: response 0,15, dampingFraction 0,86 (для жеста, ведущего элемент).
    pub fn interactive() -> Self {
        Self::from_response_unchecked(INTERACTIVE_SPRING_RESPONSE_S, INTERACTIVE_SPRING_DAMPING_FRACTION)
    }

    /// Физические параметры; `None` для [`Spring::immediate`] (у неё нет конечной жёсткости).
    pub fn physical(&self) -> Option<PhysicalSpring> {
        match self.kind {
            SpringKind::Physical(physical) => Some(physical),
            SpringKind::Immediate => None,
        }
    }

    /// Собственная угловая частота `√(k/m)`, рад/с; `None` для `immediate`.
    pub fn angular_frequency(&self) -> Option<f64> {
        self.physical().map(|p| (p.stiffness / p.mass).sqrt())
    }

    /// Это «бесконечно жёсткая» пружина?
    pub fn is_immediate(&self) -> bool {
        matches!(self.kind, SpringKind::Immediate)
    }

    /// Доля критического затухания ζ = `c / (2·√(k·m))`; `None` для `immediate`.
    pub fn damping_ratio(&self) -> Option<f64> {
        self.physical()
            .map(|p| p.damping / (2.0 * (p.stiffness * p.mass).sqrt()))
    }

    /// Обратный перевод в `duration`/`bounce` (`motion.md` 1.2). `None` для `immediate`.
    ///
    /// `duration = 2π·√(m/k)`; `bounce = 1 − ζ` при `ζ ≤ 1`, иначе `1/ζ − 1`.
    pub fn perceptual(&self) -> Option<PerceptualSpring> {
        let physical = self.physical()?;
        let duration_s = 2.0 * PI * (physical.mass / physical.stiffness).sqrt();
        let ratio = self.damping_ratio()?;
        let bounce = if ratio <= 1.0 { 1.0 - ratio } else { 1.0 / ratio - 1.0 };
        Some(PerceptualSpring { duration_s, bounce })
    }

    /// Пружина с учётом Reduce Motion (`motion.md` раздел 3: «упругость выключается»).
    ///
    /// **Единственное место правила** для аниматора свойств, движка прокрутки и токенов `ui-kit`.
    /// * `Full` — без изменений;
    /// * `Reduced`: пружина с перелётом (`bounce > 0`, затухание ниже критического) становится
    ///   критически затухающей — та же жёсткость и масса, значит та же длительность, `bounce = 0`;
    /// * пружина без перелёта (`bounce <= 0`) и [`Spring::immediate`] остаются как были:
    ///   плавность и длительность измеренных пружин камеры не трогаем.
    ///
    /// Не может завершиться ошибкой: масса и жёсткость уже проверены при построении.
    pub fn for_motion(self, motion: MotionPreference) -> Spring {
        match (motion, self.kind) {
            (MotionPreference::Reduced, SpringKind::Physical(physical)) if self.bounces() => Spring {
                kind: SpringKind::Physical(PhysicalSpring {
                    // Критическое затухание ζ = 1: `c = 2·√(k·m)`.
                    damping: 2.0 * (physical.stiffness * physical.mass).sqrt(),
                    ..physical
                }),
            },
            _ => self,
        }
    }

    /// Пружина перелетает цель (затухание ниже критического)?
    fn bounces(&self) -> bool {
        self.perceptual().is_some_and(|perceptual| perceptual.bounce > 0.0)
    }

    /// Построение без проверок — только для констант, заведомо корректных.
    fn from_duration_unchecked(duration_s: f64, bounce: f64) -> Self {
        Self::from_response_unchecked(duration_s, damping_ratio_from_bounce(bounce))
    }

    /// `k = (2π/response)²`, `c = 4π·ζ/response` при массе 1.
    fn from_response_unchecked(response_s: f64, damping_ratio: f64) -> Self {
        let angular = 2.0 * PI / response_s;
        Self {
            kind: SpringKind::Physical(PhysicalSpring {
                mass: 1.0,
                stiffness: angular * angular,
                damping: 2.0 * angular * damping_ratio,
            }),
        }
    }
}

/// ζ по `bounce` (`motion.md` 1.2).
fn damping_ratio_from_bounce(bounce: f64) -> f64 {
    if bounce >= 0.0 {
        1.0 - bounce
    } else {
        1.0 / (1.0 + bounce)
    }
}
