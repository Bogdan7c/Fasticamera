//! Прореживание входа: приборам не нужен каждый пиксель, шаг — рычаг бюджета.

use scope_contract::ScopeScan;
use serde::{Deserialize, Serialize};

/// Наибольший шаг прореживания: дальше отсчётов слишком мало для устойчивой картины.
pub const MAX_SAMPLE_STRIDE: u32 = 64;

/// Ошибка шага прореживания.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SampleStrideError {
    /// Шаг вне `1..=MAX_SAMPLE_STRIDE`.
    #[error("шаг прореживания {value} вне 1..={MAX_SAMPLE_STRIDE}")]
    OutOfRange {
        /// Недопустимое значение.
        value: u32,
    },
}

/// Шаг прореживания `≥ 1`: берётся каждый `stride`-й пиксель по обеим осям.
///
/// Берутся пиксели `x = offset + i·stride`, `y = offset + j·stride`, где `offset = (stride − 1) / 2`
/// (целочисленное деление), при условии `x < width` и `y < height`. Смещение ставит отсчёт в середину
/// блока `stride × stride`, а не в его угол. Если `offset ≥ размер` (картинка меньше половины шага),
/// отсчётов по этой оси нет: счётчики остаются нулевыми, это не ошибка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct SampleStride(u32);

impl SampleStride {
    /// Каждый пиксель.
    pub const EVERY_PIXEL: Self = Self(1);

    /// Каждый второй пиксель по обеим осям (четверть отсчётов) — значение по умолчанию `[estimate]`.
    pub const EVERY_SECOND_PIXEL: Self = Self(2);

    /// Проверенный шаг.
    pub fn try_new(value: u32) -> Result<Self, SampleStrideError> {
        if (1..=MAX_SAMPLE_STRIDE).contains(&value) {
            Ok(Self(value))
        } else {
            Err(SampleStrideError::OutOfRange { value })
        }
    }

    /// Значение шага.
    pub fn get(self) -> u32 {
        self.0
    }

    /// Смещение первого отсчёта: `(stride − 1) / 2`.
    pub fn offset(self) -> u32 {
        (self.0 - 1) / 2
    }

    /// Сколько отсчётов по оси длиной `length`: число `i` с `offset + i·stride < length`.
    pub fn samples_along(self, length: u32) -> u32 {
        let offset = self.offset();
        if offset >= length {
            0
        } else {
            (length - 1 - offset) / self.0 + 1
        }
    }

    /// Координата `index`-го отсчёта по оси.
    pub fn coordinate(self, index: u32) -> u32 {
        self.offset() + index * self.0
    }
}

impl TryFrom<u32> for SampleStride {
    type Error = SampleStrideError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::try_new(value)
    }
}

impl From<SampleStride> for u32 {
    fn from(stride: SampleStride) -> Self {
        stride.0
    }
}

/// Шаги прореживания по двум осям ПОКАЗАННОЙ картинки: `across` — по горизонтали (x), `down` — по вертикали (y).
///
/// Зачем две оси (решение 329): следу по столбцам (waveform, parade, Luma) нужны ВСЕ отсчёты вдоль взгляда — из них
/// столбцы сетки, пропуск дал бы зерно по горизонтали, — а поперёк взгляда хватает каждого второго: каждый столбец
/// получает вдвое меньше попаданий, но поровну, и вид следа (яркость — от плотности, а не от числа) тот же.
/// Какая ось поверхности «вдоль взгляда», зависит от положения телефона — это решает [`Self::for_eye`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleSteps {
    across: SampleStride,
    down: SampleStride,
}

impl SampleSteps {
    /// Один шаг по обеим осям (вектороскоп, прежнее поведение).
    pub const fn uniform(stride: SampleStride) -> Self {
        Self {
            across: stride,
            down: stride,
        }
    }

    /// Шаги по осям поверхности: `across` по x, `down` по y.
    pub const fn new(across: SampleStride, down: SampleStride) -> Self {
        Self { across, down }
    }

    /// Шаги, заданные относительно ВЗГЛЯДА: `along_eye` — вдоль оси «слева направо» сцены, `across_eye` — поперёк.
    /// При положении `scan`, в котором взгляд идёт вдоль строк поверхности (телефон боком), ось взгляда — вертикаль
    /// поверхности, иначе — горизонталь (как [`crate::SampleCount::along_eye`]).
    pub const fn for_eye(along_eye: SampleStride, across_eye: SampleStride, scan: ScopeScan) -> Self {
        if scan.runs_along_surface_rows() {
            Self::new(across_eye, along_eye)
        } else {
            Self::new(along_eye, across_eye)
        }
    }

    /// Шаг по горизонтали показанной картинки (x).
    pub const fn across(self) -> SampleStride {
        self.across
    }

    /// Шаг по вертикали показанной картинки (y).
    pub const fn down(self) -> SampleStride {
        self.down
    }

    /// Каждая ось не реже `floor`: шаг оси — наибольший из своего и `floor` (план под потолок столбцов).
    pub fn at_least(self, floor: SampleStride) -> Self {
        let raise = |stride: SampleStride| if stride.get() < floor.get() { floor } else { stride };
        Self::new(raise(self.across), raise(self.down))
    }
}

impl From<SampleStride> for SampleSteps {
    fn from(stride: SampleStride) -> Self {
        Self::uniform(stride)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_for_the_eye_follow_the_phone_orientation() {
        let (along, across) = (SampleStride::EVERY_PIXEL, SampleStride::EVERY_SECOND_PIXEL);
        // Телефон прямо и вверх ногами: взгляд по горизонтали поверхности — x вдоль, y поперёк.
        for scan in [ScopeScan::Upright, ScopeScan::UpsideDown] {
            let steps = SampleSteps::for_eye(along, across, scan);
            assert_eq!((steps.across(), steps.down()), (along, across), "{scan:?}");
        }
        // Боком: взгляд по вертикали поверхности — оси меняются местами.
        for scan in [ScopeScan::RotatedClockwise, ScopeScan::RotatedCounterClockwise] {
            let steps = SampleSteps::for_eye(along, across, scan);
            assert_eq!((steps.across(), steps.down()), (across, along), "{scan:?}");
        }
    }

    #[test]
    fn a_floor_raises_only_the_axes_below_it() {
        let steps = SampleSteps::new(SampleStride::EVERY_PIXEL, SampleStride::try_new(4).unwrap());
        let raised = steps.at_least(SampleStride::EVERY_SECOND_PIXEL);
        assert_eq!((raised.across().get(), raised.down().get()), (2, 4));
        assert_eq!(
            steps.at_least(SampleStride::EVERY_PIXEL),
            steps,
            "пол 1 ничего не меняет"
        );
    }

    #[test]
    fn stride_selects_block_centres() {
        let stride = SampleStride::try_new(4).unwrap();
        assert_eq!(stride.offset(), 1);
        let coordinates: Vec<u32> = (0..stride.samples_along(10)).map(|i| stride.coordinate(i)).collect();
        assert_eq!(coordinates, vec![1, 5, 9]);
        assert_eq!(stride.samples_along(9), 2);
        assert_eq!(stride.samples_along(1), 0);
        assert_eq!(SampleStride::EVERY_PIXEL.samples_along(7), 7);
    }

    #[test]
    fn invalid_strides_are_rejected() {
        assert_eq!(
            SampleStride::try_new(0),
            Err(SampleStrideError::OutOfRange { value: 0 })
        );
        assert!(SampleStride::try_new(MAX_SAMPLE_STRIDE + 1).is_err());
        assert!(serde_json::from_str::<SampleStride>("0").is_err());
        assert_eq!(serde_json::from_str::<SampleStride>("3").unwrap().get(), 3);
    }
}
