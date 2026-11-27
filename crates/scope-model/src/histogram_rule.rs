//! Гистограмма (сессия 14s): «уровень × канал». Правило попадания — эталон, по которому GPU считает счётчики бит в бит.
//!
//! Сетка: `bins` столбцов (УРОВЕНЬ), одна строка, плоскости — каналы R', G', B' (режимы RGB и PARADE) или одна плоскость
//! яркости Y' (режим Luma). Попадание отсчёта: плоскость, строка 0 и столбец = уровень кода этой плоскости.
//!
//! Уровень — та же целочисленная проекция «код → уровень», что у waveform и parade ([`ParadeRule`](crate::trace_rules::
//! ParadeRule), [`LumaRule`](crate::trace_rules::LumaRule)): `bin = floor(код · bins / 1024)`, серый код `c` — ровно в
//! `floor(c · bins / 1024)`, белый `1023` — в `bins − 1`, чёрный `0` — в 0. Для яркости веса BT.709 суммируются точно в
//! `bins / 1024`, поэтому серый кадр даёт один и тот же уровень во ВСЕХ плоскостях (и в Luma) без ошибки округления.
//!
//! Положение отсчёта на картинке гистограмме безразлично: считается сумма по кадру, поэтому положение телефона и
//! зеркало на счётчики не влияют (правило всегда читает картинку «прямо»).
//!
//! GLSL (на отсчёт и плоскость): `int bin = projection(code); if (bin < 0 || bin >= bins) return; hit(plane, 0, bin)`.

use scope_contract::HistogramMode;

use crate::grid::{GridError, GridShape};
use crate::projection::{IntegerProjection, PROJECTION_SHIFT, ProjectionError, WeightSum};
use crate::rule::{Hit, SamplePosition, ScopeRule};
use crate::sample_count::SampleCount;
use crate::sampling::SampleSteps;
use crate::settings::{MIN_HISTOGRAM_BINS, ScopeModelSettings};
use crate::trace_rules::{BT709_LUMA_WEIGHTS, MAX_LEVELS};

/// Ошибка построения правила гистограммы.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum HistogramRuleError {
    /// Число уровней вне `MIN_HISTOGRAM_BINS..=MAX_LEVELS`.
    #[error("уровней гистограммы {bins} вне {MIN_HISTOGRAM_BINS}..={MAX_LEVELS}")]
    BinsOutOfRange {
        /// Недопустимое число.
        bins: u32,
    },
    /// Форма сетки недопустима.
    #[error(transparent)]
    Grid(#[from] GridError),
    /// Проекцию не удалось построить.
    #[error(transparent)]
    Projection(#[from] ProjectionError),
}

/// Правило гистограммы одного режима ([`HistogramMode`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistogramRule {
    mode: HistogramMode,
    shape: GridShape,
    /// Проекции «код → уровень» по плоскостям: R', G', B' (RGB и PARADE) либо Y' в нулевой (Luma); остальные не читаются.
    projections: [IntegerProjection; 3],
}

impl HistogramRule {
    /// Правило режима `mode` с `bins` уровнями.
    pub fn new(mode: HistogramMode, bins: u32) -> Result<Self, HistogramRuleError> {
        if !(MIN_HISTOGRAM_BINS..=MAX_LEVELS).contains(&bins) {
            return Err(HistogramRuleError::BinsOutOfRange { bins });
        }
        let per_code = f64::from(bins) / 1024.0;
        let channel = |channel: usize| {
            let mut coefficients = [0.0; 3];
            coefficients[channel] = per_code;
            IntegerProjection::from_real(coefficients, 0.0, PROJECTION_SHIFT, WeightSum::Unconstrained)
        };
        let luma = IntegerProjection::from_real(
            BT709_LUMA_WEIGHTS.map(|weight| weight * per_code),
            0.0,
            PROJECTION_SHIFT,
            WeightSum::Exactly(per_code),
        )?;
        let (planes, projections) = match mode {
            HistogramMode::Rgb | HistogramMode::Parade => (3, [channel(0)?, channel(1)?, channel(2)?]),
            // Плоскость одна; две другие проекции не читаются (в блок параметров идут нулями).
            HistogramMode::Luma => (1, [luma; 3]),
        };
        Ok(Self {
            mode,
            shape: GridShape::try_new(bins, 1, planes)?,
            projections,
        })
    }

    /// Режим.
    pub fn mode(&self) -> HistogramMode {
        self.mode
    }

    /// Число уровней (столбцов сетки).
    pub fn bins(&self) -> u32 {
        self.shape.columns()
    }

    /// Проекции «код → уровень» по плоскостям правила (по одной на плоскость сетки; целые веса для GPU).
    pub fn level_projections(&self) -> &[IntegerProjection] {
        &self.projections[..self.shape.planes() as usize]
    }
}

impl ScopeRule for HistogramRule {
    fn shape(&self, _samples: SampleCount) -> GridShape {
        // Сетка от кадра не зависит: уровни те же при любом окне и шаге.
        self.shape
    }

    fn wanted_steps(&self, settings: &ScopeModelSettings) -> SampleSteps {
        SampleSteps::uniform(settings.histogram_sample_stride)
    }

    fn hits(&self, _position: SamplePosition, code: [u16; 3], report: &mut dyn FnMut(Hit)) {
        for (plane, projection) in (0u32..).zip(self.level_projections()) {
            // Уровень вне сетки (у корректных проекций невозможен) — нет попадания, а не ячейка за краем.
            let Ok(column) = u32::try_from(projection.apply(code)) else {
                continue;
            };
            if column < self.shape.columns() {
                report(Hit { plane, row: 0, column });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::hits_of_code;

    fn position() -> SamplePosition {
        SamplePosition {
            eye_index: 0,
            eye_samples: 1,
        }
    }

    #[test]
    fn modes_choose_the_number_of_planes_and_validate_the_bins() {
        assert_eq!(HistogramRule::new(HistogramMode::Rgb, 512).unwrap().shape.planes(), 3);
        assert_eq!(
            HistogramRule::new(HistogramMode::Parade, 512).unwrap().shape.planes(),
            3
        );
        let luma = HistogramRule::new(HistogramMode::Luma, 512).unwrap();
        assert_eq!(
            (luma.shape.columns(), luma.shape.rows(), luma.shape.planes()),
            (512, 1, 1)
        );
        assert_eq!(luma.level_projections().len(), 1);
        for bins in [0, 1, 1025, u32::MAX] {
            assert_eq!(
                HistogramRule::new(HistogramMode::Rgb, bins),
                Err(HistogramRuleError::BinsOutOfRange { bins })
            );
        }
    }

    #[test]
    fn a_gray_code_lands_in_the_same_level_in_every_plane_and_in_luma() {
        for bins in [2, 100, 256, 512, 1000, 1024] {
            let rgb = HistogramRule::new(HistogramMode::Rgb, bins).unwrap();
            let luma = HistogramRule::new(HistogramMode::Luma, bins).unwrap();
            for code in 0..=1023_u16 {
                let expected = u32::from(code) * bins / 1024;
                let hits = hits_of_code(&rgb, position(), [code; 3]);
                let columns: Vec<(u32, u32, u32)> = hits.iter().map(|hit| (hit.plane, hit.row, hit.column)).collect();
                assert_eq!(
                    columns,
                    vec![(0, 0, expected), (1, 0, expected), (2, 0, expected)],
                    "{bins}/{code}"
                );
                let luma_hits = hits_of_code(&luma, position(), [code; 3]);
                assert_eq!(
                    luma_hits
                        .iter()
                        .map(|hit| (hit.plane, hit.row, hit.column))
                        .collect::<Vec<_>>(),
                    vec![(0, 0, expected)],
                    "яркость серого {code} при {bins} уровнях"
                );
            }
        }
    }

    #[test]
    fn each_plane_follows_its_own_channel() {
        let rule = HistogramRule::new(HistogramMode::Parade, 512).unwrap();
        // R' = 1023, G' = 512, B' = 0 → уровни 511, 256, 0.
        let hits = hits_of_code(&rule, position(), [1023, 512, 0]);
        assert_eq!(hits.iter().map(|hit| hit.column).collect::<Vec<_>>(), vec![511, 256, 0]);
    }

    #[test]
    fn luma_weights_the_channels_like_bt709() {
        let luma = HistogramRule::new(HistogramMode::Luma, 1024).unwrap();
        let level = |code: [u16; 3]| hits_of_code(&luma, position(), code)[0].column;
        // Y' = 0,2126 R' + 0,7152 G' + 0,0722 B' на чистых каналах (при 1024 уровнях уровень = округление вниз).
        assert_eq!(level([1023, 0, 0]), (0.2126_f64 * 1023.0).floor() as u32);
        assert_eq!(level([0, 1023, 0]), (0.7152_f64 * 1023.0).floor() as u32);
        assert_eq!(level([0, 0, 1023]), (0.0722_f64 * 1023.0).floor() as u32);
        assert!(level([0, 1023, 0]) > level([1023, 0, 0]) && level([1023, 0, 0]) > level([0, 0, 1023]));
    }

    #[test]
    fn the_grid_and_the_hits_do_not_depend_on_the_frame_or_the_position() {
        let rule = HistogramRule::new(HistogramMode::Rgb, 512).unwrap();
        let small = SampleCount::for_window(10, 10, crate::SampleStride::EVERY_PIXEL);
        let large = SampleCount::for_window(3000, 2000, crate::SampleStride::EVERY_SECOND_PIXEL);
        assert_eq!(rule.shape(small), rule.shape(large));
        let first = SamplePosition {
            eye_index: 0,
            eye_samples: 5,
        };
        let last = SamplePosition {
            eye_index: 4,
            eye_samples: 5,
        };
        assert_eq!(
            hits_of_code(&rule, first, [300, 600, 900]),
            hits_of_code(&rule, last, [300, 600, 900])
        );
    }
}
