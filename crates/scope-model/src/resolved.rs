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
use crate::trace_rules::{ParadeRule, TraceRuleError, WaveformRule};
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
    /// Waveform.
    Waveform(WaveformRule),
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
            ScopeKind::Waveform => Self::Waveform(
                WaveformRule::new(settings.trace_columns, settings.trace_levels)?.with_scan(request.scan),
            ),
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

    /// Способ рисования следа этого прибора. Шаг «ветка без смены картинки» (14m): пока все приборы рисуются
    /// прежним способом — секциями.
    pub fn trace_paint(&self) -> TracePaint {
        match self {
            Self::Waveform(_) | Self::RgbParade(_) | Self::VectorscopeYCbCr(_) | Self::VectorscopeOklab(_) => {
                TracePaint::PlaneSections
            }
        }
    }

    /// Цвет следа каждой плоскости (цвета экрана).
    pub fn trace_colors(&self, settings: &RenderSettings) -> Vec<[f32; 3]> {
        match self {
            Self::Waveform(_) => vec![settings.waveform_color],
            Self::RgbParade(_) => settings.parade_colors.to_vec(),
            Self::VectorscopeYCbCr(_) | Self::VectorscopeOklab(_) => vec![settings.vectorscope_color],
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
