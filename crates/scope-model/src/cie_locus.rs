//! Спектральный локус и локус чёрного тела из таблицы CIE 1931 2° ([`crate::cie_cmf_table`], CVRL, шаг 5 нм).
//!
//! * Спектральный локус — цветность монохроматического света: `x = x̄/(x̄+ȳ+z̄)`, `y = ȳ/(x̄+ȳ+z̄)` по строкам
//!   380…700 нм. Замыкает фигуру «линия пурпурных» — отрезок от 700 к 380 нм (цвета, которых нет в спектре).
//! * Локус чёрного тела — цветность излучения по закону Планка: `X = Σ B(λ,T)·x̄` и так же `Y`, `Z` по ВСЕЙ таблице
//!   360…830 нм (у низких температур много энергии за 700 нм), `B = 1/(λ⁵·(exp(c₂/(λT)) − 1))`, `c₂ = 1,4388·10⁻² м·К`.

use isp_model::color::chromaticity::Chromaticity;

use crate::cie_cmf_table::{CMF_ROWS, CmfRow};

/// Первая длина волны контура локуса, нм.
pub const LOCUS_FIRST_NM: u16 = 380;
/// Последняя длина волны контура локуса, нм (дальше цветность почти не меняется).
pub const LOCUS_LAST_NM: u16 = 700;
/// Вторая постоянная излучения `c₂`, м·К (CIE 15).
const PLANCK_C2_METRE_KELVIN: f64 = 1.4388e-2;

fn chromaticity_of(x_bar: f64, y_bar: f64, z_bar: f64) -> Chromaticity {
    let sum = x_bar + y_bar + z_bar;
    Chromaticity {
        x: x_bar / sum,
        y: y_bar / sum,
    }
}

/// Вершины спектрального локуса 380…700 нм по возрастанию длины волны (65 точек).
pub fn spectral_locus() -> Vec<Chromaticity> {
    CMF_ROWS
        .iter()
        .filter(|row| (LOCUS_FIRST_NM..=LOCUS_LAST_NM).contains(&row.wavelength_nm))
        .map(|row| chromaticity_of(row.x_bar, row.y_bar, row.z_bar))
        .collect()
}

/// Цветность излучения чёрного тела температуры `kelvin`.
pub fn planckian_chromaticity(kelvin: f64) -> Chromaticity {
    let (mut x_sum, mut y_sum, mut z_sum) = (0.0_f64, 0.0_f64, 0.0_f64);
    for CmfRow {
        wavelength_nm,
        x_bar,
        y_bar,
        z_bar,
    } in CMF_ROWS
    {
        let wavelength_m = f64::from(wavelength_nm) * 1e-9;
        let radiance = 1.0 / (wavelength_m.powi(5) * ((PLANCK_C2_METRE_KELVIN / (wavelength_m * kelvin)).exp() - 1.0));
        x_sum += radiance * x_bar;
        y_sum += radiance * y_bar;
        z_sum += radiance * z_bar;
    }
    chromaticity_of(x_sum, y_sum, z_sum)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locus_at(wavelength_nm: u16) -> Chromaticity {
        let index = usize::from((wavelength_nm - LOCUS_FIRST_NM) / 5);
        spectral_locus()[index]
    }

    #[test]
    fn the_locus_has_one_vertex_per_five_nanometres_and_hits_the_published_points() {
        assert_eq!(spectral_locus().len(), 65);
        let green = locus_at(520);
        assert!(
            (green.x - 0.0743).abs() < 1e-4 && (green.y - 0.8338).abs() < 1e-4,
            "{green:?}"
        );
        let red = locus_at(700);
        assert!(
            (red.x - 0.7347).abs() < 1e-4 && (red.y - 0.2653).abs() < 1e-4,
            "{red:?}"
        );
    }

    #[test]
    fn planckian_points_match_the_cie_illuminant_a_and_a_6500_kelvin_body() {
        // Источник A — чёрное тело 2856 К: x = 0,44757, y = 0,40745 (CIE 15).
        let a = planckian_chromaticity(2856.0);
        assert!((a.x - 0.44757).abs() < 5e-4 && (a.y - 0.40745).abs() < 5e-4, "{a:?}");
        let cold = planckian_chromaticity(6500.0);
        assert!(
            (cold.x - 0.3135).abs() < 1e-3 && (cold.y - 0.3237).abs() < 1e-3,
            "{cold:?}"
        );
        // Чем горячее тело, тем левее и бледнее (x убывает).
        assert!(planckian_chromaticity(1667.0).x > a.x && cold.x > planckian_chromaticity(25000.0).x);
    }
}
