//! Колёса цвета по зонам: Lift (тени), Gamma (середина), Gain (света) и общее колесо Offset (решения 186, 191).
//!
//! Здесь только словарь: значения колёс и их проверка. Как колесо меняет пиксель (окна зон, сила сдвига),
//! решает `isp-model` (`look::wheels`), как колесо выглядит на экране — экран (14b).
//!
//! Каждое колесо — две независимые ручки:
//! - **толчок цвета** [`WheelPush`] — точка в единичном круге: направление — оттенок, куда толкаем цвет зоны,
//!   расстояние от центра — сила (0 — не толкаем, 1 — полная сила из конфига модели вида);
//! - **кольцо яркости** — обычная ручка [`LookKnob`] в [−1, 1]: «+» поднимает яркость зоны, «−» опускает.

use serde::{Deserialize, Serialize};

use crate::knob::LookKnob;

/// Ошибка проверки толчка цвета. Варианты различимы: экран может отличить «мусор» от «вышли за круг».
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum WheelPushError {
    /// NaN или бесконечность в одной из координат.
    #[error("координата толчка цвета не число (NaN или бесконечность)")]
    NotANumber,
    /// Точка за пределами единичного круга (сила больше 1).
    #[error("толчок цвета вне единичного круга: сила {strength}")]
    OutsideCircle {
        /// Расстояние отвергнутой точки от центра.
        strength: f32,
    },
}

/// Толчок цвета колеса: точка `(a, b)` в единичном круге, `a² + b² ≤ 1`.
///
/// Оси — оси плоскости цвета OKLab, в которой считает `isp-model`: `+a` — к пурпурно-красному,
/// `−a` — к зелёному, `+b` — к жёлтому, `−b` — к синему. Угол точки — оттенок OKLCh (0° = `+a`,
/// 90° = `+b`), его же экран 14b рисует на круге колеса.
///
/// Нейтраль одна — ровно `(0, 0)`: в полярной записи «сила 0» при любом угле дала бы бесконечно много
/// равных по смыслу, но разных по `==` значений, а значения вида сравниваются по `==` (ключ кэша
/// параметров кадра, `is_neutral`). Поэтому хранится декартова точка, а `−0.0` приводится к `0.0`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "[f32; 2]", into = "[f32; 2]")]
pub struct WheelPush {
    /// Координата по оси `a` OKLab.
    a: f32,
    /// Координата по оси `b` OKLab.
    b: f32,
}

impl WheelPush {
    /// Нейтраль: центр круга, цвет не толкаем.
    pub const NEUTRAL: WheelPush = WheelPush { a: 0.0, b: 0.0 };

    /// Проверяет и строит точку по координатам. Точка ровно на окружности (`a² + b² = 1`) допустима.
    pub fn new(a: f32, b: f32) -> Result<Self, WheelPushError> {
        if !a.is_finite() || !b.is_finite() {
            return Err(WheelPushError::NotANumber);
        }
        let squared_strength = a * a + b * b;
        if squared_strength > 1.0 {
            return Err(WheelPushError::OutsideCircle {
                strength: squared_strength.sqrt(),
            });
        }
        // `+ 0.0` превращает −0.0 в +0.0: у нейтрали одна запись и в памяти, и в файле.
        Ok(Self { a: a + 0.0, b: b + 0.0 })
    }

    /// Строит точку по углу оттенка (градусы, 0° = `+a`, 90° = `+b`) и силе.
    ///
    /// Сила за пределами [0, 1] — ошибка `OutsideCircle` (отрицательная — тоже: направление задаёт угол).
    /// Сила 0 даёт ровно [`WheelPush::NEUTRAL`] при любом угле. Округление `cos`/`sin` не может вывести
    /// точку за круг: при силе 1 она поджимается внутрь на единицу младшего разряда.
    pub fn from_hue_degrees(hue_degrees: f32, strength: f32) -> Result<Self, WheelPushError> {
        if !hue_degrees.is_finite() || !strength.is_finite() {
            return Err(WheelPushError::NotANumber);
        }
        if !(0.0..=1.0).contains(&strength) {
            return Err(WheelPushError::OutsideCircle { strength });
        }
        if strength == 0.0 {
            return Ok(Self::NEUTRAL);
        }
        let (sin, cos) = hue_degrees.to_radians().sin_cos();
        let mut a = strength * cos;
        let mut b = strength * sin;
        // `cos² + sin²` в f32 может оказаться на ULP больше 1: поджимаем, пока точка не ляжет в круг.
        while a * a + b * b > 1.0 {
            a *= 1.0 - f32::EPSILON;
            b *= 1.0 - f32::EPSILON;
        }
        Self::new(a, b)
    }

    /// Строит точку по координатам, ПРИЖИМАЯ её к единичному кругу: точка за кругом не отказ, а точка на краю
    /// в том же направлении (решение 206: палец вышел за круг — точка остаётся на краю).
    ///
    /// Внутри круга и на краю точка берётся как есть (`−0.0` приводится к `0.0`). NaN и бесконечность —
    /// `NotANumber`: направление у мусора не определено. Огромные значения (`1e30`, у которых квадрат
    /// переполнил бы `f32`) тоже прижимаются: координаты предварительно нормируются на большую из них, а не возводятся в квадрат.
    /// Результат всегда проходит проверку [`WheelPush::new`]: если округление деления оставило точку на ULP
    /// за кругом, её поджимают внутрь тем же приёмом, что и в [`WheelPush::from_hue_degrees`].
    pub fn clamped_to_circle(a: f32, b: f32) -> Result<Self, WheelPushError> {
        if !a.is_finite() || !b.is_finite() {
            return Err(WheelPushError::NotANumber);
        }
        if a * a + b * b <= 1.0 {
            return Self::new(a, b);
        }
        // Сначала делим на большую из координат (она ≠ 0, раз квадрат длины > 1): после этого обе координаты
        // в [−1, 1], и длина не переполняется даже для `f32::MAX` (у `hypot` от двух `MAX` вышла бы бесконечность).
        let largest = a.abs().max(b.abs());
        let (unit_a, unit_b) = (a / largest, b / largest);
        let length = unit_a.hypot(unit_b);
        let mut edge_a = unit_a / length;
        let mut edge_b = unit_b / length;
        while edge_a * edge_a + edge_b * edge_b > 1.0 {
            edge_a *= 1.0 - f32::EPSILON;
            edge_b *= 1.0 - f32::EPSILON;
        }
        Self::new(edge_a, edge_b)
    }

    /// Координата по оси `a` OKLab, [−1, 1].
    pub fn a(self) -> f32 {
        self.a
    }

    /// Координата по оси `b` OKLab, [−1, 1].
    pub fn b(self) -> f32 {
        self.b
    }

    /// Сила толчка — расстояние от центра, [0, 1].
    pub fn strength(self) -> f32 {
        (self.a * self.a + self.b * self.b).sqrt()
    }

    /// Точка ровно в центре.
    pub fn is_neutral(self) -> bool {
        self.a == 0.0 && self.b == 0.0
    }
}

impl Default for WheelPush {
    fn default() -> Self {
        Self::NEUTRAL
    }
}

impl TryFrom<[f32; 2]> for WheelPush {
    type Error = WheelPushError;

    fn try_from([a, b]: [f32; 2]) -> Result<Self, Self::Error> {
        Self::new(a, b)
    }
}

impl From<WheelPush> for [f32; 2] {
    fn from(push: WheelPush) -> Self {
        [push.a, push.b]
    }
}

/// Одно колесо: толчок цвета и кольцо яркости. Ручки независимы: можно крутить только одну.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColorWheel {
    /// Куда и с какой силой толкнуть цвет зоны.
    pub push: WheelPush,
    /// Кольцо яркости зоны: «+» светлее, «−» темнее.
    pub ring: LookKnob,
}

impl ColorWheel {
    /// Колесо не тронуто.
    pub const NEUTRAL: ColorWheel = ColorWheel {
        push: WheelPush::NEUTRAL,
        ring: LookKnob::NEUTRAL,
    };

    /// Обе ручки колеса в нуле.
    pub fn is_neutral(self) -> bool {
        self.push.is_neutral() && self.ring.is_neutral()
    }
}

/// Какое из четырёх колёс — для перебора колёс экраном и тестами без копирования кода на каждое поле.
///
/// Те же четыре зоны — вкладки полосок по каналам ([`crate::ChannelPrimaries`]): выбранная вкладка общая у колёс
/// и полосок (решение 343).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WheelZone {
    /// Lift — тени (темнее серого экрана).
    Lift,
    /// Gamma — середина (вокруг серого экрана).
    Gamma,
    /// Gain — света (светлее серого экрана).
    Gain,
    /// Offset — весь кадр одинаково.
    Offset,
}

impl WheelZone {
    /// Все колёса в порядке, в котором их привычно показывают (как в DaVinci Resolve).
    pub const ALL: [WheelZone; 4] = [WheelZone::Lift, WheelZone::Gamma, WheelZone::Gain, WheelZone::Offset];
}

/// Все четыре колеса вида.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColorWheels {
    /// Тени.
    pub lift: ColorWheel,
    /// Середина.
    pub gamma: ColorWheel,
    /// Света.
    pub gain: ColorWheel,
    /// Весь кадр.
    pub offset: ColorWheel,
}

impl ColorWheels {
    /// Ни одно колесо не тронуто.
    pub const NEUTRAL: ColorWheels = ColorWheels {
        lift: ColorWheel::NEUTRAL,
        gamma: ColorWheel::NEUTRAL,
        gain: ColorWheel::NEUTRAL,
        offset: ColorWheel::NEUTRAL,
    };

    /// Каждое колесо в нейтрали.
    pub fn is_neutral(&self) -> bool {
        WheelZone::ALL.iter().all(|&zone| self.wheel(zone).is_neutral())
    }

    /// Колесо зоны.
    pub fn wheel(&self, zone: WheelZone) -> ColorWheel {
        match zone {
            WheelZone::Lift => self.lift,
            WheelZone::Gamma => self.gamma,
            WheelZone::Gain => self.gain,
            WheelZone::Offset => self.offset,
        }
    }

    /// Копия, в которой колесо зоны заменено; остальные колёса не меняются.
    pub fn with_wheel(mut self, zone: WheelZone, wheel: ColorWheel) -> Self {
        match zone {
            WheelZone::Lift => self.lift = wheel,
            WheelZone::Gamma => self.gamma = wheel,
            WheelZone::Gain => self.gain = wheel,
            WheelZone::Offset => self.offset = wheel,
        }
        self
    }
}

#[cfg(test)]
#[path = "wheels_tests.rs"]
mod tests;
