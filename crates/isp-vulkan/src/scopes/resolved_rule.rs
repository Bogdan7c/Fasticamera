//! Приборы `scope-model` как [`GpuScopeRule`]: перевод решённого правила ([`ResolvedScope`]) в данные для шейдеров.
//!
//! `match` по [`ResolvedScope`] здесь — единственное место, где GPU-часть знает, какие приборы есть в эталоне
//! (как `ResolvedScope::resolve` на CPU); проходы накопления и показа его не видят.
//!
//! # Слова шейдера `scope_accumulate_integer.comp` (после общего заголовка, слова 4…)
//! | слова | смысл |
//! |---|---|
//! | 4 | источник столбца: 0 — положение отсчёта `(x·columns)/width`, 1 — проекция кода |
//! | 5, 6 | основа и знак строки: `row = основа + знак · значение` |
//! | 7…11 | проекция столбца (`w0 w1 w2 offset shift`) |
//! | 12…26 | три проекции строки по плоскостям |
//!
//! * waveform: источник 0, `основа = levels − 1`, знак −1, одна проекция уровня;
//! * parade: то же, три проекции (R, G, B);
//! * вектороскоп Y'CbCr: источник 1, основа 0, знак +1, проекции столбца и строки.
//!
//! # Слова шейдера `scope_accumulate_oklab.comp`
//! 4 — предел сетки `size` (f32), 5 — масштаб, 6 — смещение центра, 7…15 — матрица «свет → LMS» по строкам,
//! 16…18 и 19…21 — строки `a` и `b` матрицы `M2`; таблица декодирования — из правила.

use scope_contract::ScopeScan;
use scope_model::projection::IntegerProjection;
use scope_model::{GraticulePrimitive, GridShape, RenderSettings, ResolvedScope, ScopeRule};

use super::error::ScopeError;
use super::rule::{AccumulateParameters, AccumulatorId, GpuScopeRule, RuleWords};

/// Источник столбца: положение отсчёта на картинке.
const COLUMN_FROM_POSITION: u32 = 0;
/// Источник столбца: проекция кода.
const COLUMN_FROM_PROJECTION: u32 = 1;

/// Слов в одной проекции (`w0 w1 w2 offset shift`).
const PROJECTION_WORDS: usize = 5;

/// Раскладка слов целочисленного шейдера.
struct IntegerLayout<'a> {
    column_source: u32,
    row_base: i32,
    row_sign: i32,
    column_projection: Option<IntegerProjection>,
    row_projections: &'a [IntegerProjection],
}

impl IntegerLayout<'_> {
    fn into_words(self) -> RuleWords {
        let mut words = RuleWords::new();
        words
            .push_u32(self.column_source)
            .push_i32(self.row_base)
            .push_i32(self.row_sign);
        push_projection_or_zeros(&mut words, self.column_projection.as_ref());
        for plane in 0..MAX_ROW_PROJECTIONS {
            push_projection_or_zeros(&mut words, self.row_projections.get(plane));
        }
        words
    }
}

/// Сколько проекций строки (по плоскостям) читает шейдер.
const MAX_ROW_PROJECTIONS: usize = 3;

/// Проекция или нули на её месте (плоскость не используется: шейдер её не читает).
fn push_projection_or_zeros(words: &mut RuleWords, projection: Option<&IntegerProjection>) {
    match projection {
        Some(projection) => {
            words.push_projection(projection);
        }
        None => {
            for _ in 0..PROJECTION_WORDS {
                words.push_u32(0);
            }
        }
    }
}

/// Уровни waveform и parade растут вверх, а строка 0 — верх прибора: `row = (levels − 1) − уровень`.
///
/// Уровней не больше 1024 (проверено при построении правила), но число слова шейдера не подставляется молча: если
/// предел когда-нибудь нарушится, прибор отказывает типизированной ошибкой до первой команды.
fn trace_row_base(levels: u32) -> Result<i32, ScopeError> {
    i32::try_from(levels)
        .map(|levels| levels - 1)
        .map_err(|_| ScopeError::LevelsOutOfRange { levels })
}

impl GpuScopeRule for ResolvedScope {
    fn accumulator(&self) -> AccumulatorId {
        match self {
            // У waveform и parade отсчёты разбросаны по разным ячейкам: слияние в subgroup на Adreno 740 дороже
            // прямой записи, а серии подряд идущих отсчётов одной нити в регистре дешевле (замеры сессии 16a).
            // У вектороскопов соседние пиксели падают в одни и те же ячейки: им выгодно слияние в subgroup.
            Self::Waveform(_) | Self::RgbParade(_) => AccumulatorId::INTEGER_PROJECTION_RUNS,
            Self::VectorscopeYCbCr(_) => AccumulatorId::INTEGER_PROJECTION_MERGED,
            Self::VectorscopeOklab(_) => AccumulatorId::OKLAB_VECTORSCOPE_MERGED,
        }
    }

    fn grid(&self) -> GridShape {
        self.shape()
    }

    fn scan(&self) -> ScopeScan {
        ScopeRule::scan(self)
    }

    fn accumulate_parameters(&self) -> Result<AccumulateParameters<'_>, ScopeError> {
        Ok(match self {
            Self::Waveform(rule) => integer_parameters(IntegerLayout {
                column_source: COLUMN_FROM_POSITION,
                row_base: trace_row_base(rule.levels())?,
                row_sign: -1,
                column_projection: None,
                row_projections: &[rule.level_projection()],
            }),
            Self::RgbParade(rule) => integer_parameters(IntegerLayout {
                column_source: COLUMN_FROM_POSITION,
                row_base: trace_row_base(rule.levels())?,
                row_sign: -1,
                column_projection: None,
                row_projections: &rule.level_projections(),
            }),
            Self::VectorscopeYCbCr(rule) => integer_parameters(IntegerLayout {
                column_source: COLUMN_FROM_PROJECTION,
                row_base: 0,
                row_sign: 1,
                column_projection: Some(rule.column_projection()),
                row_projections: &[rule.row_projection()],
            }),
            Self::VectorscopeOklab(rule) => {
                let mut words = RuleWords::new();
                words
                    .push_f32(rule.geometry().size() as f32)
                    .push_f32(rule.scale())
                    .push_f32(rule.centre_offset());
                for row in rule.working_to_lms() {
                    for value in row {
                        words.push_f32(value);
                    }
                }
                for row in rule.lab_rows() {
                    for value in row {
                        words.push_f32(value);
                    }
                }
                AccumulateParameters {
                    words,
                    decode_table: Some(rule.decode_table().values()),
                }
            }
        })
    }

    fn trace_colors(&self, settings: &RenderSettings) -> Vec<[f32; 3]> {
        ResolvedScope::trace_colors(self, settings)
    }

    fn graticule(&self, settings: &RenderSettings) -> Vec<GraticulePrimitive> {
        ResolvedScope::graticule(self, settings)
    }
}

fn integer_parameters(layout: IntegerLayout<'_>) -> AccumulateParameters<'static> {
    AccumulateParameters {
        words: layout.into_words(),
        decode_table: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_base_is_levels_minus_one_and_an_unrepresentable_level_count_is_a_typed_error() {
        assert_eq!(trace_row_base(256).unwrap(), 255);
        assert_eq!(trace_row_base(1).unwrap(), 0);
        assert_eq!(trace_row_base(i32::MAX as u32).unwrap(), i32::MAX - 1);
        assert_eq!(
            trace_row_base(u32::MAX),
            Err(ScopeError::LevelsOutOfRange { levels: u32::MAX }),
            "вместо молчаливой подстановки i32::MAX"
        );
    }
}
