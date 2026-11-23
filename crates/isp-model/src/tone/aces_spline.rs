//! Сегментированный сплайн ACES: C5 (RRT) и C9 (выходное преобразование 48 нит).
//!
//! Источник и лицензия — в доке модуля [`super`] (`tone::aces`): коэффициенты и форма сплайна взяты из
//! Academy aces-dev v1.3 (`ACESlib.Tonescales.ctl`), авторство и условия лицензии сохранены там.
//!
//! # Форма
//! Сплайн работает в логарифмах `log10`: до `minPoint` и после `maxPoint` — прямые с заданным наклоном, между
//! `minPoint`, `midPoint`, `maxPoint` — два участка квадратичных B-сплайнов (`N` узлов, `N + 2` коэффициента).
//! Внутри участка `kc = (N − 1)·(lx − lx₀)/(lx₁ − lx₀)`, `j = ⌊kc⌋`, `t = kc − j`, коэффициенты `c₀, c₁, c₂ =
//! coefs[j..j+3]`:
//! ```text
//! log y = t²·(c₀/2 − c₁ + c₂/2) + t·(c₁ − c₀) + (c₀ + c₁)/2
//! ```
//! Это и есть `[t², t, 1] · (cf · M)` из CTL с базисной матрицей `M = [[½, −1, ½], [−1, 1, ½], [½, 0, 0]]`.
//! Сплайн непрерывен и дифференцируем (C1) в узлах: квадратичный B-сплайн.
//!
//! Две реализации одной формулы: [`SplineSpec`] в f64 (разрешение параметров на CPU: обратная функция,
//! значение и наклон в колене) и [`SplineParameters`] в f32 (то, что считает пиксель и получит шейдер).

use serde::Serialize;

/// Самый нижний вход сплайна в линейных единицах ACES: меньше — значение зажимается (`HALF_MIN` в CTL).
pub const HALF_MIN: f64 = 6.103_515_625e-5;

/// Сколько ячеек коэффициентов хранит сплайн (C9 — 10, C5 — 6; хвост C5 нулевой и не читается).
pub const COEFFICIENT_SLOTS: usize = 10;

/// Точка сплайна `(вход, выход)` в линейных единицах.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SplinePoint {
    pub x: f64,
    pub y: f64,
}

/// Сплайн в f64 (описание из CTL).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SplineSpec {
    coefficients_low: [f64; COEFFICIENT_SLOTS],
    coefficients_high: [f64; COEFFICIENT_SLOTS],
    /// `N_KNOTS` каждого из двух участков (4 у C5, 8 у C9).
    knots: usize,
    min: SplinePoint,
    mid: SplinePoint,
    max: SplinePoint,
    slope_low: f64,
    slope_high: f64,
}

/// Значение квадратичного B-сплайна на участке: формула модуля.
fn quadratic_segment(c0: f64, c1: f64, c2: f64, t: f64) -> f64 {
    t * t * (0.5 * c0 - c1 + 0.5 * c2) + t * (c1 - c0) + 0.5 * (c0 + c1)
}

impl SplineSpec {
    /// RRT: сплайн C5, `RRT_PARAMS` из `ACESlib.Tonescales.ctl`.
    pub fn rrt() -> Self {
        Self {
            coefficients_low: [
                -4.000_000_000_0,
                -4.000_000_000_0,
                -3.157_376_577_3,
                -0.485_249_995_8,
                1.847_732_470_6,
                1.847_732_470_6,
                0.0,
                0.0,
                0.0,
                0.0,
            ],
            coefficients_high: [
                -0.718_548_242_5,
                2.081_030_717_2,
                3.668_124_123_7,
                4.0,
                4.0,
                4.0,
                0.0,
                0.0,
                0.0,
                0.0,
            ],
            knots: 4,
            min: SplinePoint {
                x: 0.18 * (-15.0_f64).exp2(),
                y: 0.0001,
            },
            mid: SplinePoint { x: 0.18, y: 4.8 },
            max: SplinePoint {
                x: 0.18 * 18.0_f64.exp2(),
                y: 10_000.0,
            },
            slope_low: 0.0,
            slope_high: 0.0,
        }
    }

    /// Выходное преобразование: сплайн C9, `ODT_48nits` (его точки заданы через значения RRT `rrt`).
    pub fn odt_48_nits(rrt: &Self) -> Self {
        Self {
            coefficients_low: [
                -1.698_970_004_3,
                -1.698_970_004_3,
                -1.477_9,
                -1.229_1,
                -0.864_8,
                -0.448_0,
                0.005_18,
                0.451_108_033_4,
                0.911_374_441_4,
                0.911_374_441_4,
            ],
            coefficients_high: [
                0.515_438_696_5,
                0.847_043_778_3,
                1.135_8,
                1.380_2,
                1.519_7,
                1.598_5,
                1.646_7,
                1.674_609_135_7,
                1.687_873_339_0,
                1.687_873_339_0,
            ],
            knots: 8,
            min: SplinePoint {
                x: rrt.forward(0.18 * (-6.5_f64).exp2()),
                y: 0.02,
            },
            mid: SplinePoint {
                x: rrt.forward(0.18),
                y: 4.8,
            },
            max: SplinePoint {
                x: rrt.forward(0.18 * 6.5_f64.exp2()),
                y: 48.0,
            },
            slope_low: 0.0,
            slope_high: 0.04,
        }
    }

    /// Прямая функция в логарифмах: `log10 x → log10 y` (без зажима входа).
    pub fn forward_log10(&self, log_x: f64) -> f64 {
        let (log_min_x, log_mid_x, log_max_x) = (self.min.x.log10(), self.mid.x.log10(), self.max.x.log10());
        if log_x <= log_min_x {
            log_x * self.slope_low + (self.min.y.log10() - self.slope_low * log_min_x)
        } else if log_x < log_mid_x {
            self.segment(&self.coefficients_low, (log_x - log_min_x) / (log_mid_x - log_min_x))
        } else if log_x < log_max_x {
            self.segment(&self.coefficients_high, (log_x - log_mid_x) / (log_max_x - log_mid_x))
        } else {
            log_x * self.slope_high + (self.max.y.log10() - self.slope_high * log_max_x)
        }
    }

    /// Участок сплайна: `fraction ∈ [0, 1)` — доля пути по участку.
    fn segment(&self, coefficients: &[f64; COEFFICIENT_SLOTS], fraction: f64) -> f64 {
        let knot_coordinate = (self.knots - 1) as f64 * fraction;
        // Округление у правого края может дать ровно N − 1: берём последний участок при t = 1 (то же значение).
        let index = (knot_coordinate as usize).min(self.knots - 2);
        let t = knot_coordinate - index as f64;
        quadratic_segment(coefficients[index], coefficients[index + 1], coefficients[index + 2], t)
    }

    /// Прямая функция в линейных единицах: вход зажимается снизу до [`HALF_MIN`], как в CTL.
    pub fn forward(&self, x: f64) -> f64 {
        10.0_f64.powf(self.forward_log10(x.max(HALF_MIN).log10()))
    }

    /// Обратная функция в логарифмах: `log10 y → log10 x`. Вне `[minPoint.y, maxPoint.y]` — края сплайна.
    pub fn reverse_log10(&self, log_y: f64) -> f64 {
        let (log_min_y, log_mid_y, log_max_y) = (self.min.y.log10(), self.mid.y.log10(), self.max.y.log10());
        if log_y <= log_min_y {
            self.min.x.log10()
        } else if log_y <= log_mid_y {
            self.invert_segment(&self.coefficients_low, log_y, self.min.x.log10(), self.mid.x.log10())
        } else if log_y < log_max_y {
            self.invert_segment(&self.coefficients_high, log_y, self.mid.x.log10(), self.max.x.log10())
        } else {
            self.max.x.log10()
        }
    }

    /// Обращение участка: сначала участок по значениям в узлах `(c[i] + c[i+1])/2`, потом корень квадратного
    /// уравнения (как `segmented_spline_*_rev` в CTL).
    fn invert_segment(
        &self,
        coefficients: &[f64; COEFFICIENT_SLOTS],
        log_y: f64,
        log_x_start: f64,
        log_x_end: f64,
    ) -> f64 {
        let knot_value = |index: usize| 0.5 * (coefficients[index] + coefficients[index + 1]);
        let last_segment = self.knots - 2;
        let segment = (0..=last_segment)
            .find(|&index| log_y <= knot_value(index + 1))
            .unwrap_or(last_segment);
        let (c0, c1, c2) = (
            coefficients[segment],
            coefficients[segment + 1],
            coefficients[segment + 2],
        );
        let a = 0.5 * c0 - c1 + 0.5 * c2;
        let b = c1 - c0;
        let c = 0.5 * (c0 + c1) - log_y;
        let discriminant = (b * b - 4.0 * a * c).max(0.0).sqrt();
        let t = (2.0 * c) / (-discriminant - b);
        let increment = (log_x_end - log_x_start) / (self.knots - 1) as f64;
        log_x_start + (t + segment as f64) * increment
    }

    /// Обратная функция в линейных единицах.
    pub fn reverse(&self, y: f64) -> f64 {
        10.0_f64.powf(self.reverse_log10(y.max(1e-10).log10()))
    }

    /// Те же числа в f32 для пикселя и шейдера.
    pub fn parameters(&self) -> SplineParameters {
        let to_f32 = |slots: &[f64; COEFFICIENT_SLOTS]| slots.map(|value| value as f32);
        let (log_min_x, log_mid_x, log_max_x) = (self.min.x.log10(), self.mid.x.log10(), self.max.x.log10());
        let segments = (self.knots - 1) as f64;
        SplineParameters {
            coefficients_low: to_f32(&self.coefficients_low),
            coefficients_high: to_f32(&self.coefficients_high),
            log_min_x: log_min_x as f32,
            log_mid_x: log_mid_x as f32,
            log_max_x: log_max_x as f32,
            low_scale: (segments / (log_mid_x - log_min_x)) as f32,
            high_scale: (segments / (log_max_x - log_mid_x)) as f32,
            last_segment: (self.knots - 2) as u32,
            slope_low: self.slope_low as f32,
            slope_high: self.slope_high as f32,
            low_intercept: (self.min.y.log10() - self.slope_low * log_min_x) as f32,
            high_intercept: (self.max.y.log10() - self.slope_high * log_max_x) as f32,
        }
    }
}

/// Сплайн в f32: ровно те числа, что получает шейдер.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SplineParameters {
    pub coefficients_low: [f32; COEFFICIENT_SLOTS],
    pub coefficients_high: [f32; COEFFICIENT_SLOTS],
    /// `log10` точек `minPoint.x`, `midPoint.x`, `maxPoint.x`.
    pub log_min_x: f32,
    pub log_mid_x: f32,
    pub log_max_x: f32,
    /// `(N − 1)/(log_mid_x − log_min_x)` и `(N − 1)/(log_max_x − log_mid_x)`: узловая координата за один `log10`.
    pub low_scale: f32,
    pub high_scale: f32,
    /// Номер последнего участка, `N − 2`.
    pub last_segment: u32,
    /// Наклон прямых за краями (в `log10`–`log10`).
    pub slope_low: f32,
    pub slope_high: f32,
    /// Свободные члены прямых за краями: `log10(min.y) − slope_low·log_min_x` и аналогично сверху.
    pub low_intercept: f32,
    pub high_intercept: f32,
}

impl SplineParameters {
    /// `log10 x → log10 y` в f32 (формула модуля).
    pub fn forward_log10(&self, log_x: f32) -> f32 {
        if log_x <= self.log_min_x {
            log_x * self.slope_low + self.low_intercept
        } else if log_x < self.log_mid_x {
            self.segment(&self.coefficients_low, (log_x - self.log_min_x) * self.low_scale)
        } else if log_x < self.log_max_x {
            self.segment(&self.coefficients_high, (log_x - self.log_mid_x) * self.high_scale)
        } else {
            log_x * self.slope_high + self.high_intercept
        }
    }

    /// Участок: `knot_coordinate = (N − 1)·доля`.
    fn segment(&self, coefficients: &[f32; COEFFICIENT_SLOTS], knot_coordinate: f32) -> f32 {
        let index = (knot_coordinate as u32).min(self.last_segment);
        let t = knot_coordinate - index as f32;
        let index = index as usize;
        let (c0, c1, c2) = (coefficients[index], coefficients[index + 1], coefficients[index + 2]);
        t * t * (0.5 * c0 - c1 + 0.5 * c2) + t * (c1 - c0) + 0.5 * (c0 + c1)
    }
}
