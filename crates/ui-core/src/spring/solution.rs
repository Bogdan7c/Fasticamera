//! Аналитическое решение `m·x'' + c·x' + k·x = 0` (`motion.md` 1.3).
//!
//! Все три режима (недо-, критическое и перезатухание) записаны одной формулой через две
//! «базисные» функции, поэтому около ζ = 1 нет деления на √|1−ζ²|:
//!
//! ```text
//! a = c / 2m,  ω0² = k / m,  q = ω0² − a²   (q > 0 колебания, q < 0 перезатухание)
//! C(t) = cos(√q·t)        S(t) = sin(√q·t) / √q      (при q < 0 — cosh и sinh)
//! x(t) = e^(−a·t) · ( x0·C + (v0 + a·x0)·S )
//! v(t) = e^(−a·t) · ( v0·C − S·(ω0²·x0 + a·v0) )
//! ```
//!
//! `C` и `S` — целые функции от `w = q·t²`: при малом `|w|` считаются рядом Тейлора (непрерывно
//! проходит через q = 0), иначе — прямыми `cos/sin` или экспонентами.

use super::model::{PhysicalSpring, Spring};

/// Смещение от цели и скорость. Единицы — как у величины анимации (pt и pt/с).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpringState {
    /// Смещение от точки равновесия (цели): `значение − цель`.
    pub displacement: f64,
    pub velocity: f64,
}

/// Ниже этого `|q·t²|` используется ряд Тейлора. При 0,25 восьми членов хватает до ~1e-15.
const SERIES_THRESHOLD: f64 = 0.25;
/// Число членов ряда Тейлора.
const SERIES_TERMS: usize = 8;

impl Spring {
    /// Состояние через `elapsed_s` секунд от `initial`. Отрицательное время считается нулём.
    ///
    /// Для [`Spring::immediate`] смещение и скорость сразу нулевые (элемент уже на цели).
    /// При `bounce = 1` (затухание 0) колебание не затухает никогда.
    pub fn evolve(&self, initial: SpringState, elapsed_s: f64) -> SpringState {
        match self.physical() {
            Some(physical) => evolve_physical(physical, initial, elapsed_s.max(0.0)),
            None => SpringState {
                displacement: 0.0,
                velocity: 0.0,
            },
        }
    }
}

fn evolve_physical(spring: PhysicalSpring, initial: SpringState, elapsed_s: f64) -> SpringState {
    let decay_rate = spring.damping / (2.0 * spring.mass);
    let omega_squared = spring.stiffness / spring.mass;
    let (decayed_cos, decayed_sinc) = decayed_basis(decay_rate, omega_squared, elapsed_s);

    let displacement =
        initial.displacement * decayed_cos + (initial.velocity + decay_rate * initial.displacement) * decayed_sinc;
    let velocity = initial.velocity * decayed_cos
        - decayed_sinc * (omega_squared * initial.displacement + decay_rate * initial.velocity);
    SpringState { displacement, velocity }
}

/// `(e^(−a·t)·C(t), e^(−a·t)·S(t))`.
fn decayed_basis(decay_rate: f64, omega_squared: f64, elapsed_s: f64) -> (f64, f64) {
    let q = omega_squared - decay_rate * decay_rate;
    let w = q * elapsed_s * elapsed_s;
    if w.abs() < SERIES_THRESHOLD {
        let envelope = (-decay_rate * elapsed_s).exp();
        let (cos_like, sinc_like) = series_basis(w);
        return (envelope * cos_like, envelope * elapsed_s * sinc_like);
    }
    if w > 0.0 {
        let envelope = (-decay_rate * elapsed_s).exp();
        let root = q.sqrt();
        return (
            envelope * (root * elapsed_s).cos(),
            envelope * (root * elapsed_s).sin() / root,
        );
    }
    // Перезатухание: cosh/sinh раскрыты в экспоненты, чтобы не переполнять `cosh` при большом t.
    // Медленный показатель a − s = ω0²/(a + s) считается без вычитания близких чисел.
    let root = (-q).sqrt();
    let slow_rate = omega_squared / (decay_rate + root);
    let fast_rate = decay_rate + root;
    let slow = (-slow_rate * elapsed_s).exp();
    let fast = (-fast_rate * elapsed_s).exp();
    (0.5 * (slow + fast), (slow - fast) / (2.0 * root))
}

/// Ряды `cos(√w) = Σ (−w)^n/(2n)!` и `sin(√w)/√w = Σ (−w)^n/(2n+1)!` (верны и при w < 0).
fn series_basis(w: f64) -> (f64, f64) {
    let mut cos_term = 1.0;
    let mut sinc_term = 1.0;
    let mut cos_sum = 1.0;
    let mut sinc_sum = 1.0;
    for n in 0..SERIES_TERMS {
        let n = n as f64;
        cos_term *= -w / ((2.0 * n + 1.0) * (2.0 * n + 2.0));
        sinc_term *= -w / ((2.0 * n + 2.0) * (2.0 * n + 3.0));
        cos_sum += cos_term;
        sinc_sum += sinc_term;
    }
    (cos_sum, sinc_sum)
}
