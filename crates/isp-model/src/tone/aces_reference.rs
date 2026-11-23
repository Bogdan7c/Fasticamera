//! Независимый эталон ACES в f64 для тестов: сплайны и шаги RRT/ODT переписаны с текста CTL (aces-dev v1.3)
//! по-другому, чем в основе: базисная матрица `M` перемножается явно, поиск участков и обращение — как в
//! `segmented_spline_*_rev`, сдвиг экспозиции ищется бисекцией по ПРЯМОЙ кривой (в основе — по обратной).
//! Матрицы AP0/AP1 — опубликованные числа ACES (10 знаков), а не выведенные из координат цветности.

use super::settings::AcesSettings;
use crate::color::chromaticity::bradford_adaptation;
use crate::color::matrix::Matrix3;
use crate::color::working_space::WORKING_PRIMARIES;

pub const HALF_MIN: f64 = 6.103_515_625e-5;

/// Базисная матрица квадратичного B-сплайна из `ACESlib.Tonescales.ctl`.
const M: [[f64; 3]; 3] = [[0.5, -1.0, 0.5], [-1.0, 1.0, 0.5], [0.5, 0.0, 0.0]];

/// AP0 → AP1 (опубликованные числа ACES).
pub const AP0_TO_AP1: [[f64; 3]; 3] = [
    [1.451_439_316_1, -0.236_510_746_9, -0.214_928_569_3],
    [-0.076_553_773_4, 1.176_229_699_8, -0.099_675_926_4],
    [0.008_316_148_4, -0.006_032_449_8, 0.997_716_301_4],
];
/// AP1 → XYZ (опубликованные числа ACES, белая D60).
pub const AP1_TO_XYZ: [[f64; 3]; 3] = [
    [0.662_454_181_1, 0.134_004_206_5, 0.156_187_687_0],
    [0.272_228_716_8, 0.674_081_765_8, 0.053_689_517_4],
    [-0.005_574_649_5, 0.004_060_733_5, 1.010_339_100_3],
];
/// AP0 → XYZ (опубликованные числа ACES).
pub const AP0_TO_XYZ: [[f64; 3]; 3] = [
    [0.952_552_395_9, 0.0, 0.000_093_678_6],
    [0.343_966_449_8, 0.728_166_096_6, -0.072_132_546_4],
    [0.0, 0.0, 1.008_825_184_4],
];

/// Сплайн в записи CTL.
pub struct CtlSpline {
    low: Vec<f64>,
    high: Vec<f64>,
    knots: usize,
    min: (f64, f64),
    mid: (f64, f64),
    max: (f64, f64),
    slope_low: f64,
    slope_high: f64,
}

fn times_basis(cf: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|k| cf[0] * M[0][k] + cf[1] * M[1][k] + cf[2] * M[2][k])
}

impl CtlSpline {
    pub fn c5() -> Self {
        Self {
            low: vec![-4.0, -4.0, -3.1573765773, -0.4852499958, 1.8477324706, 1.8477324706],
            high: vec![-0.7185482425, 2.0810307172, 3.6681241237, 4.0, 4.0, 4.0],
            knots: 4,
            min: (0.18 * 2.0_f64.powf(-15.0), 0.0001),
            mid: (0.18, 4.8),
            max: (0.18 * 2.0_f64.powf(18.0), 10000.0),
            slope_low: 0.0,
            slope_high: 0.0,
        }
    }

    pub fn c9(c5: &Self) -> Self {
        Self {
            low: vec![
                -1.6989700043,
                -1.6989700043,
                -1.4779000000,
                -1.2291000000,
                -0.8648000000,
                -0.4480000000,
                0.0051800000,
                0.4511080334,
                0.9113744414,
                0.9113744414,
            ],
            high: vec![
                0.5154386965,
                0.8470437783,
                1.1358000000,
                1.3802000000,
                1.5197000000,
                1.5985000000,
                1.6467000000,
                1.6746091357,
                1.6878733390,
                1.6878733390,
            ],
            knots: 8,
            min: (c5.forward(0.18 * 2.0_f64.powf(-6.5)), 0.02),
            mid: (c5.forward(0.18), 4.8),
            max: (c5.forward(0.18 * 2.0_f64.powf(6.5)), 48.0),
            slope_low: 0.0,
            slope_high: 0.04,
        }
    }

    pub fn min(&self) -> (f64, f64) {
        self.min
    }
    pub fn mid(&self) -> (f64, f64) {
        self.mid
    }
    pub fn max(&self) -> (f64, f64) {
        self.max
    }

    /// `segmented_spline_*_fwd` из CTL.
    pub fn forward(&self, x: f64) -> f64 {
        let log_x = x.max(HALF_MIN).log10();
        let (lmin, lmid, lmax) = (self.min.0.log10(), self.mid.0.log10(), self.max.0.log10());
        let knots = (self.knots - 1) as f64;
        let on_spline = |coefs: &[f64], knot_coord: f64| {
            let j = knot_coord as usize;
            let t = knot_coord - j as f64;
            let cf = [coefs[j], coefs[j + 1], coefs[j + 2]];
            let row = times_basis(cf);
            t * t * row[0] + t * row[1] + row[2]
        };
        let log_y = if log_x <= lmin {
            log_x * self.slope_low + (self.min.1.log10() - self.slope_low * lmin)
        } else if log_x > lmin && log_x < lmid {
            on_spline(&self.low, knots * (log_x - lmin) / (lmid - lmin))
        } else if log_x >= lmid && log_x < lmax {
            on_spline(&self.high, knots * (log_x - lmid) / (lmax - lmid))
        } else {
            log_x * self.slope_high + (self.max.1.log10() - self.slope_high * lmax)
        };
        10.0_f64.powf(log_y)
    }

    /// `segmented_spline_*_rev` из CTL.
    pub fn reverse(&self, y: f64) -> f64 {
        let knots = self.knots;
        let increment = |from: f64, to: f64| (to - from) / (knots as f64 - 1.0);
        let knot_y = |coefs: &[f64]| -> Vec<f64> { (0..knots).map(|i| (coefs[i] + coefs[i + 1]) / 2.0).collect() };
        let log_y = y.max(1e-10).log10();
        let solve = |coefs: &[f64], ky: &[f64], base: f64, step: f64| {
            let mut chosen = 0;
            for j in 0..knots - 1 {
                if log_y > ky[j] && log_y <= ky[j + 1] {
                    chosen = j;
                }
            }
            let row = times_basis([coefs[chosen], coefs[chosen + 1], coefs[chosen + 2]]);
            let (a, b, c) = (row[0], row[1], row[2] - log_y);
            let d = (b * b - 4.0 * a * c).sqrt();
            let t = (2.0 * c) / (-d - b);
            base + (t + chosen as f64) * step
        };
        let log_x = if log_y <= self.min.1.log10() {
            self.min.0.log10()
        } else if log_y > self.min.1.log10() && log_y <= self.mid.1.log10() {
            solve(
                &self.low,
                &knot_y(&self.low),
                self.min.0.log10(),
                increment(self.min.0.log10(), self.mid.0.log10()),
            )
        } else if log_y > self.mid.1.log10() && log_y < self.max.1.log10() {
            solve(
                &self.high,
                &knot_y(&self.high),
                self.mid.0.log10(),
                increment(self.mid.0.log10(), self.max.0.log10()),
            )
        } else {
            self.max.0.log10()
        };
        10.0_f64.powf(log_x)
    }
}

fn rows_times(matrix: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|row| matrix[row][0] * v[0] + matrix[row][1] * v[1] + matrix[row][2] * v[2])
}

/// `rgb_2_saturation`.
fn saturation(rgb: [f64; 3]) -> f64 {
    let (largest, smallest) = (rgb[0].max(rgb[1]).max(rgb[2]), rgb[0].min(rgb[1]).min(rgb[2]));
    (largest.max(1e-10) - smallest.max(1e-10)) / largest.max(1e-2)
}

/// `rgb_2_yc`.
fn yc(rgb: [f64; 3]) -> f64 {
    let [r, g, b] = rgb;
    let chroma = (b * (b - g) + g * (g - r) + r * (r - b)).max(0.0).sqrt();
    (b + g + r + 1.75 * chroma) / 3.0
}

fn sigmoid(x: f64) -> f64 {
    let t = (1.0 - (x / 2.0).abs()).max(0.0);
    (1.0 + x.signum() * (1.0 - t * t)) / 2.0
}

fn glow_fwd(yc_in: f64, gain: f64, mid: f64) -> f64 {
    if yc_in <= 2.0 / 3.0 * mid {
        gain
    } else if yc_in >= 2.0 * mid {
        0.0
    } else {
        gain * (mid / yc_in - 0.5)
    }
}

/// `cubic_basis_shaper` с явной матрицей `M` CTL (4×4).
fn cubic_basis_shaper(x: f64, w: f64) -> f64 {
    let basis = [
        [-1.0 / 6.0, 3.0 / 6.0, -3.0 / 6.0, 1.0 / 6.0],
        [3.0 / 6.0, -6.0 / 6.0, 3.0 / 6.0, 0.0],
        [-3.0 / 6.0, 0.0, 3.0 / 6.0, 0.0],
        [1.0 / 6.0, 4.0 / 6.0, 1.0 / 6.0, 0.0],
    ];
    let knots = [-w / 2.0, -w / 4.0, 0.0, w / 4.0, w / 2.0];
    if !(x > knots[0] && x < knots[4]) {
        return 0.0;
    }
    let knot_coord = (x - knots[0]) * 4.0 / w;
    let j = knot_coord as usize;
    let t = knot_coord - j as f64;
    let monomials = [t * t * t, t * t, t, 1.0];
    let column = 3 - j;
    let y: f64 = (0..4).map(|row| monomials[row] * basis[row][column]).sum();
    y * 1.5
}

/// Свечение и красный модификатор RRT (`RRT.ctl`) для ACES2065-1 с настоящим `atan2`.
pub fn sweeteners(settings: &AcesSettings, aces: [f64; 3]) -> [f64; 3] {
    let sat = saturation(aces);
    let s = sigmoid((sat - 0.4) / 0.2);
    let glow = 1.0 + glow_fwd(yc(aces), f64::from(settings.glow_gain) * s, 0.08);
    let mut out = aces.map(|c| glow * c);
    let weight = if out[0] == out[1] && out[1] == out[2] {
        0.0
    } else {
        let mut hue = (3.0_f64.sqrt() * (out[1] - out[2]))
            .atan2(2.0 * out[0] - out[1] - out[2])
            .to_degrees();
        if hue < 0.0 {
            hue += 360.0;
        }
        let centered = if hue > 180.0 { hue - 360.0 } else { hue };
        cubic_basis_shaper(centered, 135.0)
    };
    out[0] += weight * sat * (0.03 - out[0]) * (1.0 - f64::from(settings.red_modifier_scale));
    out
}

/// Полный эталон основы ACES: кривая, колено, матрицы.
pub struct Reference {
    pub settings: AcesSettings,
    c5: CtlSpline,
    c9: CtlSpline,
    pub shift_ev: f64,
    pub knee_value: f64,
    pub knee_slope: f64,
    working_to_ap0: Matrix3,
    working_from_xyz_d65: Matrix3,
    d60_to_d65: Matrix3,
}

impl Reference {
    pub fn new(settings: &AcesSettings) -> Self {
        let c5 = CtlSpline::c5();
        let c9 = CtlSpline::c9(&c5);
        let working_to_xyz = WORKING_PRIMARIES.rgb_to_xyz().unwrap();
        let ap0_xyz = Matrix3::from_rows(AP0_TO_XYZ);
        let d65_to_d60 = bradford_adaptation(WORKING_PRIMARIES.white, crate::tone::aces::AP0.white).unwrap();
        let d60_to_d65 = bradford_adaptation(crate::tone::aces::AP0.white, WORKING_PRIMARIES.white).unwrap();
        let mut reference = Self {
            settings: *settings,
            c5,
            c9,
            shift_ev: 0.0,
            knee_value: 0.0,
            knee_slope: 0.0,
            working_to_ap0: ap0_xyz.inverse().unwrap().mul(&d65_to_d60).mul(&working_to_xyz),
            working_from_xyz_d65: working_to_xyz.inverse().unwrap(),
            d60_to_d65,
        };
        // Сдвиг экспозиции бисекцией: нейтраль на серой карте со сдвигом даёт заданный серый экрана.
        let target = f64::from(settings.display_middle_gray);
        let (mut low, mut high) = (-3.0, 3.0);
        for _ in 0..80 {
            let middle = 0.5 * (low + high);
            if reference.neutral_display(middle) < target {
                low = middle;
            } else {
                high = middle;
            }
        }
        reference.shift_ev = 0.5 * (low + high);
        let knee = f64::from(settings.knee_ev);
        let probe = 1e-5;
        reference.knee_value = reference.aces_cv(knee);
        reference.knee_slope = (reference.aces_cv(knee + probe) - reference.aces_cv(knee - probe)) / (2.0 * probe);
        reference
    }

    /// `linCV` нейтральной компоненты при входе `0,18·2^(e + δ)` (кривая ACES, без колена).
    fn aces_cv(&self, exposure: f64) -> f64 {
        let x = 0.18 * (exposure + self.shift_ev).exp2();
        let nits = self.c9.forward(self.c5.forward(x));
        let black = 10.0_f64.powf(0.02_f64.log10());
        ((nits - black) / (48.0 - black)).max(0.0)
    }

    /// Серый экрана нейтрали при сдвиге `shift` (для бисекции): `linCV^γ_s`.
    fn neutral_display(&self, shift: f64) -> f64 {
        let x = 0.18 * shift.exp2();
        let nits = self.c9.forward(self.c5.forward(x));
        let black = 10.0_f64.powf(0.02_f64.log10());
        ((nits - black) / (48.0 - black))
            .max(0.0)
            .powf(f64::from(self.settings.surround_gamma))
    }

    fn shape(&self, e: f64) -> f64 {
        let s = &self.settings;
        let bump = e * e / (e * e + f64::from(s.shape_width_ev).powi(2));
        let sweep = if e < 0.0 { s.shadows_ev } else { s.highlights_ev };
        f64::from(s.contrast) * e + f64::from(sweep) * bump
    }

    /// Белый точки в ступенях (с пределом монотонности кубики).
    pub fn white_ev(&self, white: f64) -> f64 {
        let grey = f64::from(self.settings.scene_middle_gray);
        let cap = f64::from(self.settings.knee_ev) + 3.0 * (1.0 - self.knee_value) / self.knee_slope;
        self.shape((white.max(1.0) / grey).log2()).min(cap)
    }

    /// `linCV` канала AP1 со своим белым.
    pub fn channel(&self, value: f64, white: f64) -> f64 {
        let e1 = self.shape((value.max(HALF_MIN) / 0.18).log2());
        let knee = f64::from(self.settings.knee_ev);
        let white_ev = self.white_ev(white);
        if e1 >= white_ev {
            return 1.0;
        }
        if e1 >= knee {
            let w = white_ev - knee;
            let t = (e1 - knee) / w;
            let h00 = 2.0 * t.powi(3) - 3.0 * t * t + 1.0;
            let h10 = t.powi(3) - 2.0 * t * t + t;
            let h01 = -2.0 * t.powi(3) + 3.0 * t * t;
            return h00 * self.knee_value + h10 * w * self.knee_slope + h01;
        }
        self.aces_cv(e1)
    }

    /// Пиксель сцены в W → линейный свет экрана в W (f64, без сужения матриц).
    pub fn pixel(&self, scene: [f64; 3], white: f64) -> [f64; 3] {
        let s = &self.settings;
        let scale = 0.18 / f64::from(s.scene_middle_gray);
        let aces = scene.map(|c| (c * scale).clamp(0.0, 65504.0));
        let ap0 = self.working_to_ap0.apply(aces);
        let sweetened = sweeteners(s, ap0).map(|c| c.clamp(0.0, 65504.0));
        let ap1 = rows_times(&AP0_TO_AP1, sweetened).map(|c| c.clamp(0.0, 65504.0));
        // Глобальная десатурация RRT: `calc_sat_adjust_matrix`.
        let y_row = AP1_TO_XYZ[1];
        let rrt_sat = f64::from(s.rrt_saturation);
        let luminance: f64 = (0..3).map(|i| y_row[i] * ap1[i]).sum();
        let ap1 = ap1.map(|c| rrt_sat * c + (1.0 - rrt_sat) * luminance);
        let cv = ap1.map(|c| self.channel(c, white));
        // Поправка тусклого окружения через xyY (как `darkSurround_to_dimSurround`).
        let xyz = rows_times(&AP1_TO_XYZ, cv);
        let sum = xyz[0] + xyz[1] + xyz[2];
        let dimmed_xyz = if sum <= 0.0 {
            xyz
        } else {
            let (x, y, big_y) = (xyz[0] / sum, xyz[1] / sum, xyz[1].max(0.0));
            let big_y = big_y.powf(f64::from(s.surround_gamma));
            [x * big_y / y, big_y, (1.0 - x - y) * big_y / y]
        };
        let inverse_ap1 = Matrix3::from_rows(AP1_TO_XYZ).inverse().unwrap();
        let ap1_dimmed = inverse_ap1.apply(dimmed_xyz);
        // Десатурация выхода и перевод в W.
        let odt_sat = f64::from(s.odt_saturation);
        let luminance: f64 = (0..3).map(|i| y_row[i] * ap1_dimmed[i]).sum();
        let ap1_final = ap1_dimmed.map(|c| odt_sat * c + (1.0 - odt_sat) * luminance);
        let xyz_d60 = rows_times(&AP1_TO_XYZ, ap1_final);
        let xyz_d65 = self.d60_to_d65.apply(xyz_d60);
        self.working_from_xyz_d65.apply(xyz_d65)
    }
}
