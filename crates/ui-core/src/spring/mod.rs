//! Пружины iOS: физическая модель, аналитическое решение, анимации с прерыванием.
//!
//! Спецификация — `docs/ios-camera/motion.md` разделы 1.1–1.5. Состав:
//! * [`Spring`] — модель «масса — жёсткость — затухание» с конструкторами из пар Apple
//!   (`duration`+`bounce`, `response`+`dampingFraction`) и пресетами `smooth/snappy/bouncy`;
//! * [`SpringState`] и [`Spring::evolve`] — точное решение уравнения пружины от начального
//!   смещения и скорости (недо-, критическое и перезатухание, устойчиво около ζ = 1);
//! * [`SpringAnimation`] — скалярная анимация, прерывание по правилу «замена» (SwiftUI `spring`);
//! * [`AdditiveSpringAnimation`] — прерывание по правилу «сложение» (SwiftUI `interpolatingSpring`);
//! * [`RestCriterion`] — когда считать пружину успокоившейся;
//! * [`SpringSpec`] — сериализуемое описание пружины для конфигов.
//!
//! Время приходит снаружи (`MonotonicNanos`), положение считается от «сейчас − старт», а не
//! шагами `dt`: выборки на 60 и 120 Гц в общих моментах совпадают побитово.

mod additive;
mod animation;
mod model;
mod rest;
mod solution;
mod spec;

pub use additive::{AdditiveSpringAnimation, AdditiveSpringStart, DEFAULT_MAX_LIVE_CONTRIBUTIONS};
pub use animation::{
    SpringAnimation, SpringPhase, SpringSample, SpringStart, distance_fraction_per_second,
    velocity_from_distance_fraction,
};
pub use model::{PerceptualSpring, PhysicalSpring, Spring, SpringError};
pub use rest::{RestCriterion, RestTolerance};
pub use solution::SpringState;
pub use spec::SpringSpec;
