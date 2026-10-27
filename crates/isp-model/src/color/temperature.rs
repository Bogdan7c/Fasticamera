//! Коррелированная цветовая температура (CCT) по методу Робертсона и температуры
//! эталонных источников света DNG.
//!
//! DNG интерполирует цветовые матрицы между двумя калибровками по температуре белого,
//! поэтому нужна функция «цветность → кельвины». Повторяем поведение Adobe DNG SDK
//! (`dng_temperature::Set_xy_coord`): та же таблица изотемпературных линий и те же правила
//! на краях. Метод: A. R. Robertson, «Computation of Correlated Color Temperature and
//! Distribution Temperature», JOSA 58 (1968), стр. 1528–1535.

use camera_contract::capabilities::ReferenceIlluminant;

use super::chromaticity::Chromaticity;

/// Температура в кельвинах.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Kelvin(pub f64);

impl Kelvin {
    /// Обратная температура в майредах (микрообратные кельвины): `10⁶ / T`.
    /// Интерполяция DNG линейна именно по майредам, а не по кельвинам.
    pub fn mired(self) -> f64 {
        MIRED_NUMERATOR / self.0
    }
}

/// Числитель перевода кельвины ↔ майреды: 1 майред = 10⁶ / К.
const MIRED_NUMERATOR: f64 = 1.0e6;

/// Строка таблицы Робертсона: точка на кривой Планка в цветовом пространстве CIE 1960 UCS (u, v)
/// и наклон изотемпературной линии (нормали к кривой) в этой точке.
#[derive(Debug, Clone, Copy)]
struct IsotemperatureLine {
    /// Обратная температура, майреды.
    mired: f64,
    u: f64,
    v: f64,
    /// Наклон `dv/du` изотемпературной линии.
    slope: f64,
}

/// Таблица Робертсона 1968: 31 линия от 0 до 600 майред (∞ … 1667 К).
/// Совпадает со значениями Adobe DNG SDK (`kTempTable`) и, проверено по числам,
/// с `colour.temperature.robertson1968.DATA_ISOTEMPERATURE_LINES_ROBERTSON1968` (colour-science 0.4.7):
/// строки сгенерированы скриптом из этого массива.
const ISOTEMPERATURE_LINES: [IsotemperatureLine; 31] = [
    IsotemperatureLine {
        mired: 0.0,
        u: 0.18006,
        v: 0.26352,
        slope: -0.24341,
    },
    IsotemperatureLine {
        mired: 10.0,
        u: 0.18066,
        v: 0.26589,
        slope: -0.25479,
    },
    IsotemperatureLine {
        mired: 20.0,
        u: 0.18133,
        v: 0.26846,
        slope: -0.26876,
    },
    IsotemperatureLine {
        mired: 30.0,
        u: 0.18208,
        v: 0.27119,
        slope: -0.28539,
    },
    IsotemperatureLine {
        mired: 40.0,
        u: 0.18293,
        v: 0.27407,
        slope: -0.3047,
    },
    IsotemperatureLine {
        mired: 50.0,
        u: 0.18388,
        v: 0.27709,
        slope: -0.32675,
    },
    IsotemperatureLine {
        mired: 60.0,
        u: 0.18494,
        v: 0.28021,
        slope: -0.35156,
    },
    IsotemperatureLine {
        mired: 70.0,
        u: 0.18611,
        v: 0.28342,
        slope: -0.37915,
    },
    IsotemperatureLine {
        mired: 80.0,
        u: 0.1874,
        v: 0.28668,
        slope: -0.40955,
    },
    IsotemperatureLine {
        mired: 90.0,
        u: 0.1888,
        v: 0.28997,
        slope: -0.44278,
    },
    IsotemperatureLine {
        mired: 100.0,
        u: 0.19032,
        v: 0.29326,
        slope: -0.47888,
    },
    IsotemperatureLine {
        mired: 125.0,
        u: 0.19462,
        v: 0.30141,
        slope: -0.58204,
    },
    IsotemperatureLine {
        mired: 150.0,
        u: 0.19962,
        v: 0.30921,
        slope: -0.70471,
    },
    IsotemperatureLine {
        mired: 175.0,
        u: 0.20525,
        v: 0.31647,
        slope: -0.84901,
    },
    IsotemperatureLine {
        mired: 200.0,
        u: 0.21142,
        v: 0.32312,
        slope: -1.0182,
    },
    IsotemperatureLine {
        mired: 225.0,
        u: 0.21807,
        v: 0.32909,
        slope: -1.2168,
    },
    IsotemperatureLine {
        mired: 250.0,
        u: 0.22511,
        v: 0.33439,
        slope: -1.4512,
    },
    IsotemperatureLine {
        mired: 275.0,
        u: 0.23247,
        v: 0.33904,
        slope: -1.7298,
    },
    IsotemperatureLine {
        mired: 300.0,
        u: 0.2401,
        v: 0.34308,
        slope: -2.0637,
    },
    IsotemperatureLine {
        mired: 325.0,
        u: 0.24792,
        v: 0.34655,
        slope: -2.4681,
    },
    IsotemperatureLine {
        mired: 350.0,
        u: 0.25591,
        v: 0.34951,
        slope: -2.9641,
    },
    IsotemperatureLine {
        mired: 375.0,
        u: 0.264,
        v: 0.352,
        slope: -3.5814,
    },
    IsotemperatureLine {
        mired: 400.0,
        u: 0.27218,
        v: 0.35407,
        slope: -4.3633,
    },
    IsotemperatureLine {
        mired: 425.0,
        u: 0.28039,
        v: 0.35577,
        slope: -5.3762,
    },
    IsotemperatureLine {
        mired: 450.0,
        u: 0.28863,
        v: 0.35714,
        slope: -6.7262,
    },
    IsotemperatureLine {
        mired: 475.0,
        u: 0.29685,
        v: 0.35823,
        slope: -8.5955,
    },
    IsotemperatureLine {
        mired: 500.0,
        u: 0.30505,
        v: 0.35907,
        slope: -11.324,
    },
    IsotemperatureLine {
        mired: 525.0,
        u: 0.3132,
        v: 0.35968,
        slope: -15.628,
    },
    IsotemperatureLine {
        mired: 550.0,
        u: 0.32129,
        v: 0.36011,
        slope: -23.325,
    },
    IsotemperatureLine {
        mired: 575.0,
        u: 0.32931,
        v: 0.36038,
        slope: -40.77,
    },
    IsotemperatureLine {
        mired: 600.0,
        u: 0.33724,
        v: 0.36051,
        slope: -116.45,
    },
];

/// Индекс последней линии таблицы (600 майред ≈ 1667 К).
const LAST_LINE_INDEX: usize = ISOTEMPERATURE_LINES.len() - 1;

/// Температура по цветности — алгоритм `Set_xy_coord` из Adobe DNG SDK.
///
/// 1. xy → (u, v) CIE 1960: `u = 2x / (1,5 − x + 6y)`, `v = 3y / (1,5 − x + 6y)`.
/// 2. Идём по линиям 1…30, считаем знаковое расстояние точки до линии; первая линия,
///    под которой (расстояние ≤ 0) оказалась точка, и предыдущая образуют пару.
/// 3. Майреды интерполируются линейно по весу `f = dt / (dt_prev + dt)`.
///
/// Края (как в SDK, а не ошибка):
/// - холоднее 600 майред (краснее ≈ 1667 К) — прижимается к последней линии, ответ 1666,7 К;
/// - синее первой линии (10 майред) — ответ 10 майред = 100 000 К;
/// - нефинитный вход (или `1,5 − x + 6y ≤ 0`) даёт `Kelvin(NaN)`: вызывающий код обязан
///   отсеивать такие цветности (в `dng` это делает `Chromaticity::from_xyz`).
pub fn temperature_from_chromaticity(xy: Chromaticity) -> Kelvin {
    let denominator = 1.5 - xy.x + 6.0 * xy.y;
    if !denominator.is_finite() || denominator <= 0.0 {
        return Kelvin(f64::NAN);
    }
    let u = 2.0 * xy.x / denominator;
    let v = 3.0 * xy.y / denominator;

    // Расстояние до предыдущей линии (над ней, > 0); для индекса 1 не используется.
    let mut previous_distance = 0.0;
    for (index, line) in ISOTEMPERATURE_LINES.iter().enumerate().skip(1) {
        // Единичный направляющий вектор линии: (1, slope) / длина.
        let length = (1.0 + line.slope * line.slope).sqrt();
        let (direction_u, direction_v) = (1.0 / length, line.slope / length);
        // Знаковое расстояние от точки до линии (векторное произведение).
        let distance = -(u - line.u) * direction_v + (v - line.v) * direction_u;
        if distance <= 0.0 || index == LAST_LINE_INDEX {
            // Правее последней линии прижимаем к ней (SDK: `if (dt > 0) dt = 0`).
            let below_distance = (-distance).max(0.0);
            // На первой линии вес 0: ответ — её собственные майреды.
            let previous_weight = if index == 1 {
                0.0
            } else {
                below_distance / (previous_distance + below_distance)
            };
            let previous_mired = ISOTEMPERATURE_LINES[index - 1].mired;
            let mired = previous_mired * previous_weight + line.mired * (1.0 - previous_weight);
            return Kelvin(MIRED_NUMERATOR / mired);
        }
        previous_distance = distance;
    }
    // Недостижимо: на индексе LAST_LINE_INDEX цикл всегда возвращается.
    Kelvin(f64::NAN)
}

/// Эталонный свет с кодом EXIF, которого нет в таблице температур DNG (0 «неизвестно», 255 «другой»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("для эталонного света с кодом EXIF LightSource {exif_code} нет температуры в таблице DNG")]
pub struct UnknownCalibrationIlluminant {
    pub exif_code: u8,
}

/// CCT, которую Adobe DNG SDK приписывает коду EXIF LightSource
/// (`dng_camera_profile::IlluminantToTemperature`). Для люминесцентных ламп берётся середина
/// диапазона из названия кода. Значения сверены с таблицей `CCT_ILLUMINANTS_ADOBEDNG` из
/// colour-hdri 0.2.6, которая сама ссылается на SDK. Исходники SDK недоступны офлайн, поэтому
/// различия с ними (в частности для кодов 15 и 16, где в колоночных названиях EXIF разночтения)
/// не проверены.
pub fn calibration_illuminant_temperature(
    illuminant: ReferenceIlluminant,
) -> Result<Kelvin, UnknownCalibrationIlluminant> {
    let exif_code = illuminant.exif_code();
    let kelvin = match exif_code {
        // 17 стандартный свет A, 3 лампа накаливания.
        17 | 3 => 2850.0,
        // 24 студийный вольфрам ISO.
        24 => 3200.0,
        // 23 D50.
        23 => 5000.0,
        // 20 D55, 1 дневной свет, 9 ясная погода, 4 вспышка, 18 стандартный свет B.
        20 | 1 | 9 | 4 | 18 => 5500.0,
        // 21 D65, 19 стандартный свет C, 10 облачно.
        21 | 19 | 10 => 6500.0,
        // 22 D75, 11 тень.
        22 | 11 => 7500.0,
        // 12 дневная люминесцентная D (5700–7100 К).
        12 => (5700.0 + 7100.0) * 0.5,
        // 13 дневная белая N (4600–5400 К).
        13 => (4600.0 + 5400.0) * 0.5,
        // 14 холодная белая W (3900–4500 К) и 2 «люминесцентная» без уточнения.
        14 | 2 => (3900.0 + 4500.0) * 0.5,
        // 15 белая WW (3200–3700 К).
        15 => (3200.0 + 3700.0) * 0.5,
        // 16 тёплая белая L (2600–3250 К).
        16 => (2600.0 + 3250.0) * 0.5,
        _ => return Err(UnknownCalibrationIlluminant { exif_code }),
    };
    Ok(Kelvin(kelvin))
}

#[cfg(test)]
#[path = "temperature_tests.rs"]
mod tests;
