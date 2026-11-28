//! Полоски Lift / Gamma / Gain / Offset по каналам R, G, B (решения 339–343).
//!
//! Работают над светом экрана `W` ПОСЛЕ тона вида и ДО колёс/насыщенности/полос ([`super::color`]): в Resolve
//! «Primaries» — первая коррекция (решение 341). Лишних переводов пространства нет: полоски идут в RGB между
//! стадией тона и переводом в OKLab.
//!
//! # Каналы (решение 342)
//! Каналы R, G, B полосок — основные цвета Display P3 (настройка [`ChannelBasis`]): полоска R двигает только след R
//! на parade видоискателя. `W` — рабочее пространство ОСНОВЫ (у AgX/ACES/filmic — Rec. 2020, у прежней основы —
//! цвета выхода), поэтому матрицы строятся от `working`, а не от константы:
//! ```text
//! rgb_basis = M_W→basis · rgb_W,   M_W→basis = XYZ_from_RGB(basis)⁻¹ · XYZ_from_RGB(W)   (белая D65 с обеих сторон)
//! rgb_W'    = M_basis→W · rgb_basis'
//! ```
//! Если основа каналов совпадает с `W` (прежняя основа на экране P3, либо `channel_basis = working`), обе матрицы —
//! ТОЧНО единичные, умножение даёт тот же результат бит в бит.
//!
//! # Формула одного канала `x` (свет экрана в основе каналов; может быть чуть < 0 и > 1)
//! 1. **Кодирование** с линейным «носком» (нечётная функция, `c` — `encoding_toe`):
//!    ```text
//!    e = sign(x)·(√(|x| + c) − √c) / n,   n = √(1 + c) − √c        e(0) = 0, e(1) = 1
//!    ```
//!    У чистого √x наклон в нуле бесконечный: Lift/Offset усиливали бы шум и расхождение CPU↔GPU у каналов около
//!    нуля, а в основе P3 у каждого насыщенного цвета есть канал около или ниже нуля. С `c > 0` наклон в нуле
//!    конечен (`1 / (2·√c·n)`), функция C1 и обратима.
//! 2. **Gamma** — рациональная кривая без степени, `s = 2^(gamma_slope_stops · ручка)`:
//!    ```text
//!    γ(e) = s·e / (1 + (s − 1)·e)   при 0 ≤ e ≤ 1;   s·e при e < 0;   1 + (e − 1)/s при e > 1
//!    ```
//!    `γ(0) = 0`, `γ(1) = 1`, `γ'(0) = s`, `γ'(1) = 1/s`; вне отрезка продолжение линейное с наклонами концов (C1).
//! 3. **Lift и Gain** — линейная интерполяция «чёрное → белое»: `e₂ = l·(1 − e₁) + G·e₁`, где
//!    `l = lift_range·ручка`, `G = 2^(gain_stops_in_sqrt·ручка)`. Чёрное → `l`, белое → `G`; Gamma чёрное и белое не
//!    двигает, поэтому Lift тянет чёрное и сходит на нет к белому, Gain — наоборот (зоны перекрываются, решение 340).
//! 4. **Offset**: `e₃ = e₂ + offset_range·ручка` (равный сдвиг в кодировке).
//! 5. **Декодирование** — обратная функция: `x = sign(e)·t·(t + 2√c)`, `t = |e|·n`. Это то же самое, что
//!    `((|e|·n + √c)² − c)`, но без вычитания близких чисел: в нуле результат ровно 0, а не `√c² − c ≈ 10⁻⁹`.
//!
//! Нейтраль (все 12 ручек в нуле) — шаг пропускается целиком ([`LookPrimaries::resolve`] даёт `None`), картинка бит
//! в бит как без полосок. На пиксель: 6 корней, 3 деления, 2 матрицы 3×3.
//!
//! # Монотонность
//! Кодирование и декодирование строго растут, Gamma растёт при любом `s > 0`, Offset — сдвиг. Остаётся шаг 3:
//! `de₂/de₁ = G − l`. Наихудшее сочетание краёв — `G_min = 2^(−gain_stops_in_sqrt)` и `l_max = lift_range`, поэтому
//! конфиг обязан держать `2^(−gain_stops_in_sqrt) > lift_range` ([`PrimariesModelSettings::validate`]). Тогда
//! светлое не становится темнее тёмного ни на одном сочетании краёв всех 12 ручек.

use look_contract::{ChannelPrimaries, ChannelTriple, PrimaryChannel, WheelZone};
use serde::{Deserialize, Serialize};

use crate::color::chromaticity::Primaries;
use crate::color::matrix::{Matrix3, PixelMatrix, SingularMatrix};

/// Основа каналов R, G, B полосок (решение 342).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelBasis {
    /// Основные цвета Display P3: каналы совпадают со следами parade видоискателя на экране P3.
    #[default]
    DisplayP3,
    /// Рабочее пространство основы тона `W`: перевода нет (задел под видео в Rec. 2020 / HDR).
    Working,
}

/// Верхняя граница `encoding_toe`: дальше кодирование вырождается в почти линейное.
const MAX_ENCODING_TOE: f32 = 1.0;
/// Верхняя граница `lift_range`: Lift обязан оставаться много меньше Gain (проверка монотонности строже).
const MAX_LIFT_RANGE: f32 = 1.0;
/// Верхняя граница `gamma_slope_stops`: `2^3 = 8` — наклон середины, дальше дробь теряет точность f32 у края.
const MAX_GAMMA_SLOPE_STOPS: f32 = 3.0;
/// Верхняя граница `gain_stops_in_sqrt`: `2^2 = 4` в кодировке — ×16 света, больше не нужно.
const MAX_GAIN_STOPS_IN_SQRT: f32 = 2.0;
/// Верхняя граница `offset_range`: полшкалы кодирования.
const MAX_OFFSET_RANGE: f32 = 0.5;

/// Конфиг полосок (`render.viewfinder.look_model.primaries`). Силы краёв выбраны владельцем по листам 14t
/// («сильно ×2»); «носок» кодирования — `[estimate]`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PrimariesModelSettings {
    /// Основа каналов R, G, B.
    pub channel_basis: ChannelBasis,
    /// `c`: «носок» кодирования (см. модуль), строго больше нуля.
    pub encoding_toe: f32,
    /// Сила Lift на краю ручки (±1), в шкале кодирования.
    pub lift_range: f32,
    /// Сила Gamma на краю: `log2` наклона середины `s`.
    pub gamma_slope_stops: f32,
    /// Сила Gain на краю: `log2` множителя белого `G` в шкале кодирования.
    pub gain_stops_in_sqrt: f32,
    /// Сила Offset на краю, в шкале кодирования.
    pub offset_range: f32,
}

impl Default for PrimariesModelSettings {
    fn default() -> Self {
        // Силы краёв — выбор владельца по листам 14t: «сильно ×2» от первых оценок (0,10 / 0,5 / 0,25 / 0,05).
        // Монотонность держится: 2^(−0,5) ≈ 0,707 > 0,20 (см. `validate`).
        Self {
            channel_basis: ChannelBasis::DisplayP3,
            encoding_toe: 0.01,
            lift_range: 0.20,
            gamma_slope_stops: 1.0,
            gain_stops_in_sqrt: 0.5,
            offset_range: 0.10,
        }
    }
}

/// Конфиг полосок не годится. Варианты различимы: «не число», «носок не положителен», «вне диапазона»,
/// «Lift ломает монотонность».
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum InvalidPrimariesModel {
    /// Число не конечное (NaN или бесконечность).
    #[error("primaries.{field} = {value}: нужно конечное число")]
    NotFinite {
        /// Поле конфига.
        field: &'static str,
        /// Отвергнутое значение.
        value: f32,
    },
    /// «Носок» кодирования не положителен: у кодирования был бы бесконечный наклон в нуле.
    #[error("primaries.encoding_toe = {value}: нужно строго больше нуля")]
    EncodingToeNotPositive {
        /// Отвергнутое значение.
        value: f32,
    },
    /// Число вне диапазона `0…max` (или `(0, max]` у носка).
    #[error("primaries.{field} = {value}: нужно в диапазоне {min}…{max}")]
    OutOfRange {
        /// Поле конфига.
        field: &'static str,
        /// Отвергнутое значение.
        value: f32,
        /// Нижняя граница (включительно).
        min: f32,
        /// Верхняя граница (включительно).
        max: f32,
    },
    /// `2^(−gain_stops_in_sqrt) ≤ lift_range`: на краях ручек чёрное оказалось бы светлее белого.
    #[error(
        "primaries.lift_range = {lift_range}: при gain_stops_in_sqrt = {gain_stops_in_sqrt} нужно меньше {gain_floor}"
    )]
    BreaksMonotonicity {
        /// Отвергнутая сила Lift.
        lift_range: f32,
        /// Сила Gain из конфига.
        gain_stops_in_sqrt: f32,
        /// Строгий предел для Lift: `2^(−gain_stops_in_sqrt)`.
        gain_floor: f32,
    },
}

impl PrimariesModelSettings {
    /// Проверяет числа и монотонность на краях ручек (см. «Монотонность» в доке модуля).
    pub fn validate(&self) -> Result<(), InvalidPrimariesModel> {
        let finite = |field: &'static str, value: f32| {
            if value.is_finite() {
                Ok(())
            } else {
                Err(InvalidPrimariesModel::NotFinite { field, value })
            }
        };
        let in_range = |field: &'static str, value: f32, max: f32| {
            if (0.0..=max).contains(&value) {
                Ok(())
            } else {
                Err(InvalidPrimariesModel::OutOfRange {
                    field,
                    value,
                    min: 0.0,
                    max,
                })
            }
        };
        // Конечность — отдельным проходом, чтобы NaN не получил сообщение «вне диапазона».
        finite("encoding_toe", self.encoding_toe)?;
        finite("lift_range", self.lift_range)?;
        finite("gamma_slope_stops", self.gamma_slope_stops)?;
        finite("gain_stops_in_sqrt", self.gain_stops_in_sqrt)?;
        finite("offset_range", self.offset_range)?;
        if self.encoding_toe <= 0.0 {
            return Err(InvalidPrimariesModel::EncodingToeNotPositive {
                value: self.encoding_toe,
            });
        }
        if self.encoding_toe > MAX_ENCODING_TOE {
            return Err(InvalidPrimariesModel::OutOfRange {
                field: "encoding_toe",
                value: self.encoding_toe,
                min: 0.0,
                max: MAX_ENCODING_TOE,
            });
        }
        in_range("lift_range", self.lift_range, MAX_LIFT_RANGE)?;
        in_range("gamma_slope_stops", self.gamma_slope_stops, MAX_GAMMA_SLOPE_STOPS)?;
        in_range("gain_stops_in_sqrt", self.gain_stops_in_sqrt, MAX_GAIN_STOPS_IN_SQRT)?;
        in_range("offset_range", self.offset_range, MAX_OFFSET_RANGE)?;
        let gain_floor = (-self.gain_stops_in_sqrt).exp2();
        if self.lift_range >= gain_floor {
            return Err(InvalidPrimariesModel::BreaksMonotonicity {
                lift_range: self.lift_range,
                gain_stops_in_sqrt: self.gain_stops_in_sqrt,
                gain_floor,
            });
        }
        Ok(())
    }
}

/// Константы кодирования с «носком» (одинаковы для всех каналов; считаются при разрешении, а не на пиксель).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PrimariesEncoding {
    /// `c`: «носок».
    pub toe: f32,
    /// `√c`.
    pub toe_root: f32,
    /// `n = √(1 + c) − √c`: значение функции в единице до нормировки.
    pub scale: f32,
    /// `1/n`.
    pub inverse_scale: f32,
}

impl PrimariesEncoding {
    /// Константы для носка `toe` (считаются в f64, сужаются один раз — GPU кладёт эти же f32 в uniform).
    pub fn new(toe: f32) -> Self {
        let toe_wide = f64::from(toe);
        let scale_wide = (1.0 + toe_wide).sqrt() - toe_wide.sqrt();
        Self {
            toe,
            toe_root: toe.sqrt(),
            scale: scale_wide as f32,
            inverse_scale: (1.0 / scale_wide) as f32,
        }
    }

    /// Кодирование одного канала (нечётное).
    pub fn encode(&self, light: f32) -> f32 {
        let magnitude = ((light.abs() + self.toe).sqrt() - self.toe_root) * self.inverse_scale;
        if light < 0.0 { -magnitude } else { magnitude }
    }

    /// Обратное кодирование: `t·(t + 2√c)`, `t = |e|·n` (см. пункт 5 в доке модуля).
    pub fn decode(&self, encoded: f32) -> f32 {
        let stretched = encoded.abs() * self.scale;
        let magnitude = stretched * (stretched + 2.0 * self.toe_root);
        if encoded < 0.0 { -magnitude } else { magnitude }
    }
}

/// Полоски, разрешённые в числа кадра (общий вид для CPU и GPU: эти же числа кладутся в uniform).
/// Четыре тройки — по зонам, внутри тройки порядок R, G, B.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct LookPrimaries {
    /// Константы кодирования.
    pub encoding: PrimariesEncoding,
    /// `W → основа каналов`, по строкам.
    pub working_to_basis: [[f32; 3]; 3],
    /// `основа каналов → W`, по строкам.
    pub basis_to_working: [[f32; 3]; 3],
    /// `l` по каналам: куда уходит чёрное (в шкале кодирования).
    pub lift: [f32; 3],
    /// `s` по каналам: наклон Gamma у чёрного.
    pub gamma_slope: [f32; 3],
    /// `G` по каналам: куда уходит белое (в шкале кодирования).
    pub gain: [f32; 3],
    /// `o` по каналам: равный сдвиг в кодировке.
    pub offset: [f32; 3],
}

impl LookPrimaries {
    /// Разрешает полоски для рабочего пространства `working`. `None` — все 12 ручек в нуле: шаг пропускается
    /// целиком (нейтраль бит в бит). Конфиг уже проверен ([`PrimariesModelSettings::validate`]).
    pub fn resolve(
        primaries: &ChannelPrimaries,
        model: &PrimariesModelSettings,
        working: &Primaries,
    ) -> Result<Option<Self>, SingularMatrix> {
        if primaries.is_neutral() {
            return Ok(None);
        }
        let (working_to_basis, basis_to_working) = basis_matrices(model.channel_basis, working)?;
        // По тройке: ручка каждого канала → число зоны через `amount`.
        let per_channel = |zone: WheelZone, amount: &dyn Fn(f32) -> f32| -> [f32; 3] {
            let triple: &ChannelTriple = primaries.zone(zone);
            PrimaryChannel::ALL.map(|channel| amount(triple.get(channel).get()))
        };
        Ok(Some(Self {
            encoding: PrimariesEncoding::new(model.encoding_toe),
            working_to_basis,
            basis_to_working,
            lift: per_channel(WheelZone::Lift, &|knob| model.lift_range * knob),
            gamma_slope: per_channel(WheelZone::Gamma, &|knob| (model.gamma_slope_stops * knob).exp2()),
            gain: per_channel(WheelZone::Gain, &|knob| (model.gain_stops_in_sqrt * knob).exp2()),
            offset: per_channel(WheelZone::Offset, &|knob| model.offset_range * knob),
        }))
    }

    /// Кривая одного канала `channel` (0 — R, 1 — G, 2 — B) в основе каналов: шаги 1–5 доки модуля.
    /// Тот же порядок операций, что в GLSL (`look_primaries.glsl`).
    pub fn curve(&self, channel: usize, light: f32) -> f32 {
        let encoded = self.encoding.encode(light);
        let slope = self.gamma_slope[channel];
        let gammed = if encoded < 0.0 {
            slope * encoded
        } else if encoded > 1.0 {
            1.0 + (encoded - 1.0) / slope
        } else {
            slope * encoded / (1.0 + (slope - 1.0) * encoded)
        };
        let interpolated = self.lift[channel] * (1.0 - gammed) + self.gain[channel] * gammed;
        let shifted = interpolated + self.offset[channel];
        self.encoding.decode(shifted)
    }

    /// Применяет полоски к свету `W` одного пикселя.
    pub fn apply(&self, working_rgb: [f32; 3]) -> [f32; 3] {
        let basis_rgb = PixelMatrix {
            rows: self.working_to_basis,
        }
        .apply(working_rgb);
        let moved = [
            self.curve(0, basis_rgb[0]),
            self.curve(1, basis_rgb[1]),
            self.curve(2, basis_rgb[2]),
        ];
        PixelMatrix {
            rows: self.basis_to_working,
        }
        .apply(moved)
    }
}

/// Пара матриц: `W → основа каналов` и `основа каналов → W`, по строкам.
type BasisMatrices = ([[f32; 3]; 3], [[f32; 3]; 3]);

/// Матрицы `W → основа` и обратно. Совпадающая основа — ТОЧНО единичные матрицы (а не «почти»).
fn basis_matrices(basis: ChannelBasis, working: &Primaries) -> Result<BasisMatrices, SingularMatrix> {
    let basis_primaries = match basis {
        ChannelBasis::DisplayP3 => Primaries::DISPLAY_P3,
        ChannelBasis::Working => *working,
    };
    if basis_primaries == *working {
        let identity = Matrix3::IDENTITY.to_f32().rows;
        return Ok((identity, identity));
    }
    let forward = basis_primaries.rgb_to_xyz()?.inverse()?.mul(&working.rgb_to_xyz()?);
    Ok((forward.to_f32().rows, forward.inverse()?.to_f32().rows))
}
