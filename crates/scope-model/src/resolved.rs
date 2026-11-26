//! Решённый прибор: запрос экрана + настройки + основные цвета экрана → одно конкретное правило.
//!
//! `match` по четырём вариантам здесь — реестр приборов. Оба варианта вектороскопа дают одну и ту же
//! форму сетки `size × size`, поэтому смена осей на ходу не требует других ресурсов (решение 185).
//! Новый прибор = новый вариант [`ResolvedScope`] и ветка в [`ResolvedScope::resolve`] и в трёх `match`
//! ниже; чужие правила не трогаются.

use isp_model::image::DisplayGamut;
use scope_contract::{ScopeKind, ScopeRequest, ScopeScan, VectorscopeAxes};

use crate::grid::GridShape;
use crate::oklab_rule::{OklabVectorscopeError, OklabVectorscopeRule};
use crate::render_settings::RenderSettings;
use crate::rule::{Hit, SamplePosition, ScopeRule};
use crate::settings::{ScopeModelSettings, ScopeSettingsError};
use crate::trace_paint::TracePaint;
use crate::trace_rules::{ParadeRule, TraceRuleError};
use crate::trace_style::TraceStyle;
use crate::vectorscope_ycbcr::{YCbCrVectorscopeError, YCbCrVectorscopeRule};

/// Ошибка решения прибора.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ResolveError {
    /// Настройки недопустимы.
    #[error(transparent)]
    Settings(#[from] ScopeSettingsError),
    /// Правило waveform или parade не построено.
    #[error(transparent)]
    Trace(#[from] TraceRuleError),
    /// Правило вектороскопа Y'CbCr не построено.
    #[error(transparent)]
    YCbCr(#[from] YCbCrVectorscopeError),
    /// Правило вектороскопа OKLab не построено.
    #[error(transparent)]
    Oklab(#[from] OklabVectorscopeError),
}

/// Конкретное правило прибора.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedScope {
    /// RGB waveform: то же правило трёх плоскостей, что у parade, но плоскости наложены друг на друга
    /// (решение 265), а не стоят секциями.
    Waveform(ParadeRule),
    /// RGB parade.
    RgbParade(ParadeRule),
    /// Вектороскоп, оси Y'CbCr BT.709.
    VectorscopeYCbCr(YCbCrVectorscopeRule),
    /// Вектороскоп, оси OKLab a/b.
    VectorscopeOklab(OklabVectorscopeRule),
}

impl ResolvedScope {
    /// Решает прибор запроса. Размер картинки в запросе не участвует: сетки от него не зависят.
    pub fn resolve(
        request: &ScopeRequest,
        settings: &ScopeModelSettings,
        display_gamut: DisplayGamut,
    ) -> Result<Self, ResolveError> {
        settings.validate()?;
        Ok(match request.kind {
            ScopeKind::Waveform => {
                Self::Waveform(ParadeRule::new(settings.trace_columns, settings.trace_levels)?.with_scan(request.scan))
            }
            ScopeKind::RgbParade => {
                Self::RgbParade(ParadeRule::new(settings.trace_columns, settings.trace_levels)?.with_scan(request.scan))
            }
            ScopeKind::Vectorscope => match request.vectorscope_axes {
                VectorscopeAxes::YCbCrBt709 => Self::VectorscopeYCbCr(YCbCrVectorscopeRule::new(
                    settings.vectorscope_size,
                    settings.ycbcr_gain,
                )?),
                VectorscopeAxes::OklabAb => Self::VectorscopeOklab(OklabVectorscopeRule::new(
                    settings.vectorscope_size,
                    settings.oklab_range,
                    display_gamut,
                )?),
            },
        })
    }

    /// Тип прибора.
    pub fn kind(&self) -> ScopeKind {
        match self {
            Self::Waveform(_) => ScopeKind::Waveform,
            Self::RgbParade(_) => ScopeKind::RgbParade,
            Self::VectorscopeYCbCr(_) | Self::VectorscopeOklab(_) => ScopeKind::Vectorscope,
        }
    }

    /// Как рисуется след этого прибора, вместе с данными способа (цвета, таблица цветов по месту). Единственный
    /// источник способа: эталон ([`crate::render`]) и GPU-часть спрашивают правило, а не угадывают по виду.
    ///
    /// Таблица цветов вектороскопа строится здесь по осям прибора; она зависит от правила и светлоты, но не от
    /// кадра.
    pub fn trace_style(&self, settings: &RenderSettings) -> TraceStyle {
        match self {
            Self::Waveform(_) => TraceStyle::PlanesOverlaid {
                primaries: settings.rgb_waveform_palette.primaries(),
            },
            Self::RgbParade(_) => TraceStyle::PlaneSections {
                colors: settings.parade_colors.to_vec(),
            },
            Self::VectorscopeYCbCr(rule) => TraceStyle::ColorByPosition {
                palette: Box::new(rule.position_palette(settings.vectorscope_lightness)),
            },
            Self::VectorscopeOklab(rule) => TraceStyle::ColorByPosition {
                palette: Box::new(rule.position_palette(settings.vectorscope_lightness)),
            },
        }
    }

    /// Способ рисования следа (без данных): нужен тем, кто решает по способу, не строя таблицу цветов.
    pub fn trace_paint(&self) -> TracePaint {
        match self {
            Self::Waveform(_) => TracePaint::PlanesOverlaid,
            Self::RgbParade(_) => TracePaint::PlaneSections,
            Self::VectorscopeYCbCr(_) | Self::VectorscopeOklab(_) => TracePaint::ColorByPosition,
        }
    }
}

impl ScopeRule for ResolvedScope {
    fn shape(&self) -> GridShape {
        match self {
            Self::Waveform(rule) => rule.shape(),
            Self::RgbParade(rule) => rule.shape(),
            Self::VectorscopeYCbCr(rule) => rule.shape(),
            Self::VectorscopeOklab(rule) => rule.shape(),
        }
    }

    fn scan(&self) -> ScopeScan {
        match self {
            Self::Waveform(rule) => rule.scan(),
            Self::RgbParade(rule) => rule.scan(),
            // Вектороскоп положения не меряет: поле запроса на него не влияет.
            Self::VectorscopeYCbCr(_) | Self::VectorscopeOklab(_) => ScopeScan::Upright,
        }
    }

    fn hits(&self, position: SamplePosition, code: [u16; 3], report: &mut dyn FnMut(Hit)) {
        match self {
            Self::Waveform(rule) => rule.hits(position, code, report),
            Self::RgbParade(rule) => rule.hits(position, code, report),
            Self::VectorscopeYCbCr(rule) => rule.hits(position, code, report),
            Self::VectorscopeOklab(rule) => rule.hits(position, code, report),
        }
    }
}
