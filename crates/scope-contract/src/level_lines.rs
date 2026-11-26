//! Линии уровней waveform и parade: единый список «уровень → подпись» для сетки и для экрана.
//!
//! Сетку прибора строит `scope-model` (линии кладутся туда, куда правило попадания кладёт серый код этого уровня),
//! а подписи к ним рисует экран (числа в поле слева от картинки, решение 274). Экрану нельзя знать `scope-model`
//! (решение 188), поэтому список уровней живёт в словаре: обе стороны берут его отсюда, а не заводят по копии.

/// Одна линия уровня: доля диапазона кодов и подпись к ней.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelLine {
    /// Уровень, доля диапазона кодов: 0 — чёрный, 1 — белый.
    pub fraction: f32,
    /// Подпись у линии: уровень в процентах (решение 274), без знака «%» — он стоит в заголовке поля.
    pub label: &'static str,
}

impl LevelLine {
    /// Номинальное положение линии по высоте картинки, доля от ВЕРХА (0 — верхний край, 1 — нижний): у waveform
    /// белое вверху, чёрное внизу. Настоящая линия стоит в середине строки накопителя, то есть не дальше
    /// полустроки от этого положения (проверяет `scope-model`).
    pub fn nominal_fraction_from_top(&self) -> f32 {
        1.0 - self.fraction
    }
}

/// Линии уровней waveform и parade: 0 / 25 / 50 / 75 / 100 %, снизу вверх.
pub const WAVEFORM_LEVEL_LINES: [LevelLine; 5] = [
    LevelLine {
        fraction: 0.0,
        label: "0",
    },
    LevelLine {
        fraction: 0.25,
        label: "25",
    },
    LevelLine {
        fraction: 0.5,
        label: "50",
    },
    LevelLine {
        fraction: 0.75,
        label: "75",
    },
    LevelLine {
        fraction: 1.0,
        label: "100",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_sits_at_the_top_and_black_at_the_bottom() {
        let tops: Vec<f32> = WAVEFORM_LEVEL_LINES
            .iter()
            .map(LevelLine::nominal_fraction_from_top)
            .collect();
        assert_eq!(tops, [1.0, 0.75, 0.5, 0.25, 0.0]);
    }

    #[test]
    fn labels_are_the_percentages_of_their_fractions() {
        for line in WAVEFORM_LEVEL_LINES {
            assert_eq!(line.label, format!("{}", (line.fraction * 100.0).round() as u32));
        }
    }
}
