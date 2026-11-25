//! Теплота и оттенок: сдвиг белой точки «для рисования» (решение 98, инвариант: AE и AWB видят честную базу).
//!
//! # Смысл
//! Баланс белого решает, какой свет считать нейтральным. Ручка «теплота» говорит: «считай, что свет БЫЛ чуть
//! холоднее» — тогда картинка получится теплее. Поэтому «+ теплота» СНИЖАЕТ майреды предполагаемого света
//! (его температура растёт), «+ оттенок» поднимает Duv предполагаемого света к зелёному — картинка уходит в
//! пурпурный (соглашение Lightroom: `+` оттенок = пурпурный). Знаки закреплены тестом на сером кадре.
//!
//! # Чистая функция
//! [`shift_white`] принимает только цветность и сдвиг. Она не читает ни статистику, ни DNG, ни состояние
//! AWB и ничего не пишет. Кто её вызывает, определяет, что видит сдвиг: [`crate::params::IspParams::resolve`]
//! переводит в нейтраль камеры СДВИНУТУЮ точку только для усилений баланса и матриц цвета кадра; веса яркости
//! для AE ([`crate::params::IspParams::scene_luminance_weights`]) и белая точка сцены остаются несдвинутыми.

use crate::color::chromaticity::Chromaticity;
use crate::color::dng::{CameraNeutral, ColorProfileError, DngColorProfile};
use crate::color::temperature::Kelvin;
use crate::color::white_point::{
    CctDuv, Duv, WhitePointError, cct_duv_from_chromaticity, chromaticity_from_cct_duv, table_mired_limit,
};
use crate::params::ResolveError;

/// Наименьшие майреды предполагаемого света (1 000 000 К): нулевые майреды — бесконечная температура.
const MIN_MIRED: f64 = 1.0;

/// Сдвиг белой точки: оба числа со знаком, нуль — «без сдвига».
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WhiteShift {
    /// Прибавка к майредам предполагаемого света: меньше нуля — картинка теплее.
    pub mired: f32,
    /// Прибавка на краю шкалы той же стороны, модуль: по ней ход ручки сжимается, если до края таблицы ближе.
    pub mired_edge: f32,
    /// Прибавка к Duv предполагаемого света: больше нуля — картинка пурпурнее.
    pub duv: f32,
}

impl WhiteShift {
    /// Без сдвига.
    pub const NONE: Self = Self {
        mired: 0.0,
        mired_edge: 0.0,
        duv: 0.0,
    };

    /// Оба числа равны нулю: точку не трогаем вовсе (путь бит в бит прежний).
    pub fn is_none(&self) -> bool {
        self.mired == 0.0 && self.duv == 0.0
    }
}

/// Почему сдвиг белой точки не удалось применить вовсе (кадр при этом рисуется НЕсдвинутой белой).
///
/// Ручка вида не вправе сделать кадр непригодным: белая AWB на краю таблицы Робертсона или за краем
/// интерполяции DNG не должна стоить снимка. Честная (несдвинутая) белая, если сама не переводится,
/// по-прежнему даёт ошибку `resolve` — это не дело ручки.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WhiteShiftSkipReason {
    /// Сдвиг не посчитан: белая сцены или сдвинутая белая вне таблицы Робертсона.
    ShiftOutsideTable(WhitePointError),
    /// Сдвинутая белая не переводится в нейтраль камеры по DNG-профилю.
    ShiftedWhiteHasNoNeutral(ColorProfileError),
    /// Нейтраль есть, но какой-то её канал у нуля: усиление баланса превратило бы его в шум.
    ShiftedNeutralUnusable,
    /// Матрицы цвета для сдвинутой нейтрали собрать не удалось (текст причины внутри).
    ShiftedColorUnresolvable(ResolveError),
}

/// Что стало со сдвигом белой точки: применён целиком, сжат до края таблицы, урезан до пригодного или не применён.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum WhiteShiftLimit {
    /// Сдвиг применён целиком (или сдвига нет).
    #[default]
    Unlimited,
    /// До края таблицы меньше, чем до края ручки: весь ход ручки сжат так, что край ручки — край таблицы.
    /// `edge` — сила края из модели, `available` — сколько майред есть у этого света в эту сторону.
    TravelCompressed { edge: f64, available: f64 },
    /// Запрошенная белая непригодна для этой камеры (какой-то канал нейтрали у нуля или ниже): применена
    /// ближайшая пригодная — `fraction` от оттенка, а если и без него непригодна, то от теплоты. Дальше ручка упёрлась.
    ReducedToUsable { fraction: f64 },
    /// Сдвиг не применён, кадр нарисован несдвинутой белой.
    NotApplied(WhiteShiftSkipReason),
}

/// Наименьшая доля слабейшего канала нейтрали рисования от сильнейшего. Меньше — усиление баланса свыше ×32 (5 EV):
/// канал состоит из одного шума, а у самого нуля нейтраль перестаёт быть нейтралью.
const MIN_USABLE_NEUTRAL_RATIO: f64 = 1.0 / 32.0;
/// Шагов деления пополам при поиске границы: точность 2⁻¹⁴ хода — мельче одной единицы шкалы.
const BOUNDARY_SEARCH_STEPS: u32 = 14;

/// Нейтраль рисования и то, насколько сдвиг удалось применить.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderingNeutral {
    pub neutral: CameraNeutral,
    pub limit: WhiteShiftLimit,
}

/// Нейтраль камеры для сдвинутой белой точки.
///
/// На краях ручек (±150 майред вместе с ±0,033 Duv) сдвинутая белая тёплого света выходит за то, что вообще видит
/// камера: синий канал нейтрали уходит в ноль и ниже. Отказ посреди жеста вернул бы кадр к несдвинутой белой рывком,
/// поэтому сдвиг урезается до ближайшего пригодного: сначала оттенок (теплота уже сжата до края таблицы), а если и
/// без него никак — теплота. `scene` — нейтраль самой сцены: свет, который камера уже видит, пригоден по определению.
pub fn shifted_neutral(
    profile: &DngColorProfile,
    white: Chromaticity,
    scene: CameraNeutral,
    shift: WhiteShift,
) -> Result<RenderingNeutral, WhiteShiftSkipReason> {
    let usable_ratio = MIN_USABLE_NEUTRAL_RATIO.min(channel_ratio(scene));
    let attempt = |shift: WhiteShift| -> Result<RenderingNeutral, WhiteShiftSkipReason> {
        let rendering = shift_white(white, shift).map_err(WhiteShiftSkipReason::ShiftOutsideTable)?;
        let neutral = profile
            .neutral_for_white(rendering.white)
            .map_err(WhiteShiftSkipReason::ShiftedWhiteHasNoNeutral)?;
        if channel_ratio(neutral) < usable_ratio {
            return Err(WhiteShiftSkipReason::ShiftedNeutralUnusable);
        }
        Ok(RenderingNeutral {
            neutral,
            limit: rendering.limit,
        })
    };
    let refusal = match attempt(shift) {
        Ok(full) => return Ok(full),
        Err(refusal) => refusal,
    };
    let tint_part = |fraction: f64| WhiteShift {
        duv: (f64::from(shift.duv) * fraction) as f32,
        ..shift
    };
    let warmth_part = |fraction: f64| WhiteShift {
        mired: (f64::from(shift.mired) * fraction) as f32,
        duv: 0.0,
        ..shift
    };
    let reduced = largest_usable(|fraction| attempt(tint_part(fraction)).ok())
        .or_else(|| largest_usable(|fraction| attempt(warmth_part(fraction)).ok()));
    match reduced {
        Some((fraction, rendering)) => Ok(RenderingNeutral {
            limit: WhiteShiftLimit::ReducedToUsable { fraction },
            ..rendering
        }),
        // Даже нулевой сдвиг непригоден: белая сцены вне таблицы — причина первого отказа.
        None => Err(refusal),
    }
}

/// Доля слабейшего канала нейтрали от сильнейшего.
fn channel_ratio(neutral: CameraNeutral) -> f64 {
    let channels = neutral.values();
    let strongest = channels.iter().copied().fold(f64::MIN, f64::max);
    let weakest = channels.iter().copied().fold(f64::MAX, f64::min);
    weakest / strongest
}

/// Наибольшая доля хода, при которой `attempt` удаётся (пригодные доли — отрезок от нуля). `None` — и ноль не годится.
fn largest_usable<T>(attempt: impl Fn(f64) -> Option<T>) -> Option<(f64, T)> {
    let mut best = (0.0, attempt(0.0)?);
    let mut unusable = 1.0;
    for _ in 0..BOUNDARY_SEARCH_STEPS {
        let middle = 0.5 * (best.0 + unusable);
        match attempt(middle) {
            Some(rendering) => best = (middle, rendering),
            None => unusable = middle,
        }
    }
    Some(best)
}

/// Белая точка для рисования и то, насколько сдвиг удалось применить.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderingWhite {
    pub white: Chromaticity,
    pub limit: WhiteShiftLimit,
}

/// Сдвигает белую точку по майредам вдоль локуса Планка и по Duv поперёк.
///
/// Край ручки (±150 майред) у холодного света дальше края таблицы: 8000 К — это 125 майред. Обрезка оставила бы
/// последнюю треть шкалы мёртвой, поэтому сжимается ВЕСЬ ход: ручка меняет картинку на любом свете до самого края,
/// просто на таком свете её край слабее. Что ход сжат, видно в [`RenderingWhite::limit`].
pub fn shift_white(white: Chromaticity, shift: WhiteShift) -> Result<RenderingWhite, WhitePointError> {
    let current = cct_duv_from_chromaticity(white)?;
    let current_mired = current.temperature.mired();
    let room = if shift.mired < 0.0 {
        current_mired - MIN_MIRED
    } else {
        table_mired_limit() - current_mired
    };
    let available = room.max(0.0);
    let edge = f64::from(shift.mired_edge);
    let (travel_scale, limit) = if edge <= available {
        (1.0, WhiteShiftLimit::Unlimited)
    } else {
        (available / edge, WhiteShiftLimit::TravelCompressed { edge, available })
    };
    // Прижим страхует сдвиг больше заявленного края (такой возможен только у вызывающего мимо модели вида).
    let used = (current_mired + f64::from(shift.mired) * travel_scale).clamp(MIN_MIRED, table_mired_limit());
    let shifted = chromaticity_from_cct_duv(CctDuv {
        temperature: Kelvin(1.0e6 / used),
        duv: Duv(current.duv.0 + f64::from(shift.duv)),
    })?;
    Ok(RenderingWhite { white: shifted, limit })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAYLIGHT: Chromaticity = Chromaticity::D65;

    #[test]
    fn warmer_lowers_the_assumed_light_mired_and_tint_raises_duv() {
        let base = cct_duv_from_chromaticity(DAYLIGHT).unwrap();
        let shifted = shift_white(
            DAYLIGHT,
            WhiteShift {
                mired: -40.0,
                mired_edge: 150.0,
                duv: 0.01,
            },
        )
        .unwrap();
        assert_eq!(shifted.limit, WhiteShiftLimit::Unlimited);
        let after = cct_duv_from_chromaticity(shifted.white).unwrap();
        assert!(
            (after.temperature.mired() - (base.temperature.mired() - 40.0)).abs() < 0.05,
            "{after:?}"
        );
        assert!((after.duv.0 - (base.duv.0 + 0.01)).abs() < 2e-4, "{after:?}");
    }

    #[test]
    fn zero_shift_round_trips_within_the_table_accuracy() {
        let shifted = shift_white(DAYLIGHT, WhiteShift::NONE).unwrap();
        assert!((shifted.white.x - DAYLIGHT.x).abs() < 2e-4 && (shifted.white.y - DAYLIGHT.y).abs() < 2e-4);
    }

    /// Белая после хода ручки теплоты `travel` (−1…+1, край ±150) на свете `kelvin`: цветность, её майреды (`None`,
    /// если белая вышла синее таблицы Робертсона — обратно в температуру её уже не перевести) и исход сдвига.
    fn white_after(kelvin: f64, travel: f32) -> (Chromaticity, Option<f64>, WhiteShiftLimit) {
        let light = chromaticity_from_cct_duv(CctDuv {
            temperature: Kelvin(kelvin),
            duv: Duv(0.0),
        })
        .unwrap();
        let shifted = shift_white(
            light,
            WhiteShift {
                mired: -travel * 150.0,
                mired_edge: 150.0,
                duv: 0.0,
            },
        )
        .unwrap();
        let mired = match cct_duv_from_chromaticity(shifted.white) {
            Ok(back) => Some(back.temperature.mired()),
            Err(WhitePointError::ChromaticityOutOfTable {
                edge: crate::color::white_point::TableEdge::BluerThanTable,
            }) => None,
            Err(other) => panic!("{kelvin} К, ход {travel}: {other}"),
        };
        (shifted.white, mired, shifted.limit)
    }

    fn mired_after(kelvin: f64, travel: f32) -> (f64, WhiteShiftLimit) {
        let (_, mired, limit) = white_after(kelvin, travel);
        (mired.expect("белая внутри таблицы"), limit)
    }

    /// Ручка обязана менять картинку на ВСЁМ ходу при любом свете: каждый следующий десяток — новая белая. На
    /// самом синем краю (холодный свет в сторону «теплее» упирается в 1 майред) белая может выйти синее таблицы
    /// Робертсона — тогда она всё равно должна отличаться от предыдущей, а за краем таблицы допустим только последний
    /// шаг хода.
    #[test]
    fn warmth_moves_the_white_on_the_whole_travel_under_cold_normal_and_warm_light() {
        for kelvin in [2000.0, 2850.0, 5500.0, 6600.0, 8000.0, 10_000.0] {
            let mut previous: Option<(Chromaticity, f64)> = None;
            for step in -10..=10 {
                let (white, mired, _) = white_after(kelvin, step as f32 / 10.0);
                if let Some((previous_white, previous_mired)) = previous {
                    assert_ne!(white, previous_white, "{kelvin} К, ход {step}0: белая не двинулась");
                    if let Some(mired) = mired {
                        assert!(
                            previous_mired - mired > 2.0,
                            "{kelvin} К, ход {step}0: было {previous_mired}, стало {mired}"
                        );
                    } else {
                        assert_eq!(step, 10, "{kelvin} К: белая вышла за таблицу раньше края ручки");
                    }
                }
                previous = mired.map(|mired| (white, mired));
            }
        }
    }

    #[test]
    fn travel_is_compressed_only_where_the_table_edge_is_nearer_than_the_knob_edge() {
        assert_eq!(mired_after(5500.0, 1.0).1, WhiteShiftLimit::Unlimited);
        // 8000 К = 125 майред: до края таблицы 124, край ручки 150 — ход сжат, знаменатель назван в исходе.
        let (mired, limit) = mired_after(8000.0, 0.5);
        let WhiteShiftLimit::TravelCompressed { edge, available } = limit else {
            panic!("ожидали сжатый ход: {limit:?}");
        };
        assert!(
            (edge - 150.0).abs() < 1e-9 && (available - 124.0).abs() < 0.5,
            "{limit:?}"
        );
        assert!(
            (mired - (125.0 - 62.0)).abs() < 1.0,
            "половина хода — половина доступного: {mired}"
        );
        // В другую сторону у холодного света места полно.
        assert_eq!(mired_after(8000.0, -1.0).1, WhiteShiftLimit::Unlimited);
    }

    #[test]
    fn a_white_outside_the_table_is_an_error_not_a_silent_clamp() {
        let far_blue = Chromaticity { x: 0.05, y: 0.05 };
        assert!(
            shift_white(
                far_blue,
                WhiteShift {
                    mired: -10.0,
                    mired_edge: 150.0,
                    duv: 0.0
                }
            )
            .is_err()
        );
    }
}
