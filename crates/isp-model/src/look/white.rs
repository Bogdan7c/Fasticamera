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
use crate::color::dng::ColorProfileError;
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
    /// Прибавка к Duv предполагаемого света: больше нуля — картинка пурпурнее.
    pub duv: f32,
}

impl WhiteShift {
    /// Без сдвига.
    pub const NONE: Self = Self { mired: 0.0, duv: 0.0 };

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
    /// Матрицы цвета для сдвинутой нейтрали собрать не удалось (текст причины внутри).
    ShiftedColorUnresolvable(ResolveError),
}

/// Что стало со сдвигом белой точки: применён целиком, прижат к краю таблицы или не применён вовсе.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum WhiteShiftLimit {
    /// Сдвиг применён целиком (или сдвига нет).
    #[default]
    Unlimited,
    /// Майреды ограничены краем таблицы: запрошены `requested`, применены `used`.
    MiredClamped { requested: f64, used: f64 },
    /// Сдвиг не применён, кадр нарисован несдвинутой белой.
    NotApplied(WhiteShiftSkipReason),
}

/// Белая точка для рисования и то, насколько сдвиг удалось применить.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderingWhite {
    pub white: Chromaticity,
    pub limit: WhiteShiftLimit,
}

/// Сдвигает белую точку AWB по майредам вдоль локуса Планка и по Duv поперёк. На краю таблицы майреды
/// прижимаются, и это видно в [`RenderingWhite::limit`] (кадр из-за ручки не теряется).
pub fn shift_white(white: Chromaticity, shift: WhiteShift) -> Result<RenderingWhite, WhitePointError> {
    let current = cct_duv_from_chromaticity(white)?;
    let requested = current.temperature.mired() + f64::from(shift.mired);
    let used = requested.clamp(MIN_MIRED, table_mired_limit());
    let limit = if used == requested {
        WhiteShiftLimit::Unlimited
    } else {
        WhiteShiftLimit::MiredClamped { requested, used }
    };
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

    #[test]
    fn table_edge_is_clamped_with_a_distinguishable_outcome() {
        let warm = chromaticity_from_cct_duv(CctDuv {
            temperature: Kelvin(1800.0),
            duv: Duv(0.0),
        })
        .unwrap();
        let shifted = shift_white(warm, WhiteShift { mired: 200.0, duv: 0.0 }).unwrap();
        let WhiteShiftLimit::MiredClamped { requested, used } = shifted.limit else {
            panic!("ожидали прижим к краю таблицы: {shifted:?}");
        };
        assert!(requested > used && (used - table_mired_limit()).abs() < 1e-9);
        let cold = shift_white(
            DAYLIGHT,
            WhiteShift {
                mired: -1000.0,
                duv: 0.0,
            },
        )
        .unwrap();
        assert!(matches!(cold.limit, WhiteShiftLimit::MiredClamped { used, .. } if used == MIN_MIRED));
    }

    #[test]
    fn a_white_outside_the_table_is_an_error_not_a_silent_clamp() {
        let far_blue = Chromaticity { x: 0.05, y: 0.05 };
        assert!(shift_white(far_blue, WhiteShift { mired: -10.0, duv: 0.0 }).is_err());
    }
}
