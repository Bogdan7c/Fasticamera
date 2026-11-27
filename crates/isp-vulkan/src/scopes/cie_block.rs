//! Хвост блока показа для диаграммы CIE (сессия 14s): данные шейдерного варианта `SCOPE_RENDER_CIE`.
//!
//! Раскладка (`scope_render_parameters.glsl`, нули у всех способов, кроме [`scope_model::TracePaint::Cie`]):
//! * `vec4 cie_chart`: x — левая граница окна, y — верхняя, z — сторона, w — шкала (0 xy 1931, 1 u′v′ 1976);
//! * `vec4 cie_options`: x — цвет следа (0 белый, 1 по цветности), y — заливка (0 нет, 1 бледная цветная),
//!   z — её непрозрачность, w — число рёбер локуса;
//! * `vec4 cie_xyz_to_display[3]`: строки матрицы «XYZ → линейный свет измеряемого охвата» (w не читается);
//! * `vec4 cie_locus_edges[72]`: ребро многоугольника локуса `(из.x, из.y, в.x, в.y)` в долях картинки, y вниз.

use scope_contract::CieDiagram;
use scope_model::{CieLocusFill, CieTraceColour, MAX_LOCUS_EDGES, TraceStyle};

/// Векторов `vec4` в хвосте CIE.
pub(super) const CIE_VECTORS: usize = 2 + 3 + MAX_LOCUS_EDGES;

/// Код шкалы в `cie_chart.w`.
fn diagram_code(diagram: CieDiagram) -> f32 {
    match diagram {
        CieDiagram::Xy1931 => 0.0,
        CieDiagram::UvPrime1976 => 1.0,
    }
}

/// Код цвета следа в `cie_options.x`.
fn trace_colour_code(colour: CieTraceColour) -> f32 {
    match colour {
        CieTraceColour::White => 0.0,
        CieTraceColour::ByChromaticity => 1.0,
    }
}

/// Код заливки в `cie_options.y`.
fn fill_code(fill: CieLocusFill) -> f32 {
    match fill {
        CieLocusFill::Outline => 0.0,
        CieLocusFill::FaintColour => 1.0,
    }
}

/// Слова хвоста для способа рисования `style`: у не-CIE — нули.
pub(super) fn cie_words(style: &TraceStyle) -> Vec<u32> {
    let TraceStyle::Cie(cie) = style else {
        return vec![0; CIE_VECTORS * 4];
    };
    let mut words: Vec<u32> = Vec::with_capacity(CIE_VECTORS * 4);
    words.extend(
        [
            cie.window.left,
            cie.window.top(),
            cie.window.extent,
            diagram_code(cie.diagram),
        ]
        .map(f32::to_bits),
    );
    words.extend(
        [
            trace_colour_code(cie.trace_colour),
            fill_code(cie.locus_fill),
            cie.fill_opacity,
            cie.locus_edges.len().min(MAX_LOCUS_EDGES) as f32,
        ]
        .map(f32::to_bits),
    );
    for row in cie.xyz_to_display {
        words.extend([row[0], row[1], row[2], 0.0].map(f32::to_bits));
    }
    for index in 0..MAX_LOCUS_EDGES {
        words.extend(
            cie.locus_edges
                .get(index)
                .copied()
                .unwrap_or([0.0; 4])
                .map(f32::to_bits),
        );
    }
    words
}

#[cfg(test)]
mod tests {
    use isp_model::image::DisplayGamut;
    use scope_model::{CieRenderSettings, CieRule, CieSettings, CieTraceStyle};

    use super::*;

    fn style(settings: &CieRenderSettings) -> TraceStyle {
        let rule = CieRule::new(
            CieDiagram::UvPrime1976,
            DisplayGamut::DisplayP3,
            &CieSettings::default(),
        )
        .unwrap();
        TraceStyle::Cie(Box::new(CieTraceStyle::new(&rule, settings)))
    }

    #[test]
    fn the_tail_carries_window_options_matrix_and_every_edge_and_is_zero_for_other_paints() {
        let settings = CieRenderSettings {
            trace_colour: CieTraceColour::ByChromaticity,
            locus_fill: CieLocusFill::FaintColour,
            ..CieRenderSettings::default()
        };
        let words = cie_words(&style(&settings));
        assert_eq!(words.len(), CIE_VECTORS * 4);
        let floats: Vec<f32> = words.iter().map(|word| f32::from_bits(*word)).collect();
        assert_eq!(
            &floats[..4],
            &[-0.03, scope_model::ChartWindow::UV_DEFAULT.top(), 0.68, 1.0]
        );
        assert_eq!(&floats[4..8], &[1.0, 1.0, settings.locus_fill_opacity, 65.0]);
        // Первая строка матрицы XYZ → P3: X = 2,49 R − 0,83 G − 0,40 B (известные коэффициенты обратной матрицы P3).
        assert!((floats[8] - 2.4935).abs() < 1e-3, "{}", floats[8]);
        // Рёбер 65, остальные места нули (блок заполнен целиком).
        let edges = &floats[20..];
        assert!(edges[..65 * 4].iter().any(|value| *value != 0.0));
        assert!(edges[65 * 4..].iter().all(|value| *value == 0.0));
        let other = TraceStyle::PlaneSections { colors: vec![[1.0; 3]] };
        assert!(cie_words(&other).iter().all(|word| *word == 0));
    }
}
