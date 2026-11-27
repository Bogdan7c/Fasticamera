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
use crate::position_palette::PositionPalette;
use crate::render_settings::RenderSettings;
use crate::rule::{Hit, SamplePosition, ScopeRule};
use crate::sample_count::SampleCount;
use crate::sampling::SampleSteps;
use crate::settings::{ScopeModelSettings, ScopeSettingsError};
use crate::trace_paint::TracePaint;
use crate::trace_rules::{LumaRule, ParadeRule, TraceRuleError};
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
    /// Яркостный waveform (решение 321): одна плоскость Y', белый след секцией на всю ширину.
    LumaWaveform(LumaRule),
    /// RGB parade.
    RgbParade(ParadeRule),
    /// Вектороскоп, оси Y'CbCr BT.709.
    VectorscopeYCbCr(ColouredByPosition<YCbCrVectorscopeRule>),
    /// Вектороскоп, оси OKLab a/b.
    VectorscopeOklab(ColouredByPosition<OklabVectorscopeRule>),
}

/// Правило вектороскопа вместе с таблицей цветов «по месту».
///
/// Таблицу (128 углов, у OKLab — поиск границы охвата, порядка 80 мкс на ПК) строит
/// [`ResolvedScope::resolve`] ОДИН раз вместе с правилом: она зависит от осей, охвата экрана и светлоты
/// (`vectorscope_lightness`), но не от кадра. [`ResolvedScope::trace_style`] её только отдаёт.
#[derive(Debug, Clone, PartialEq)]
pub struct ColouredByPosition<Rule> {
    /// Правило накопления вектороскопа.
    rule: Rule,
    /// Таблица цветов по углам, построенная при решении (в коробке: она заметно больше остальных полей).
    palette: Box<PositionPalette>,
}

impl<Rule> ColouredByPosition<Rule> {
    /// Правило накопления (геометрия, оси, усиление).
    pub fn rule(&self) -> &Rule {
        &self.rule
    }

    /// Таблица цветов, построенная при решении прибора.
    pub fn palette(&self) -> &PositionPalette {
        &self.palette
    }
}

impl ResolvedScope {
    /// Решает прибор запроса. Размер картинки в запросе не участвует: сетки от него не зависят (столбцы waveform/parade
    /// зависят от отсчётов кадра — см. [`ScopeRule::shape`]).
    pub fn resolve(
        request: &ScopeRequest,
        settings: &ScopeModelSettings,
        display_gamut: DisplayGamut,
    ) -> Result<Self, ResolveError> {
        settings.validate()?;
        Ok(match request.kind {
            ScopeKind::Waveform => Self::Waveform(
                ParadeRule::new(settings.trace_max_columns, settings.trace_levels)?.with_scan(request.scan),
            ),
            ScopeKind::LumaWaveform => Self::LumaWaveform(
                LumaRule::new(settings.trace_max_columns, settings.trace_levels)?.with_scan(request.scan),
            ),
            ScopeKind::RgbParade => Self::RgbParade(
                ParadeRule::new(settings.trace_max_columns, settings.trace_levels)?.with_scan(request.scan),
            ),
            ScopeKind::Vectorscope => match request.vectorscope_axes {
                // Зум ×2 (решение 325) делит цветность на краю круга: те же проходы, другие числа правила.
                VectorscopeAxes::YCbCrBt709 => {
                    let edge_chroma = settings.ycbcr_edge_chroma / request.vectorscope_zoom.magnification();
                    let gain = crate::settings::ycbcr_gain_for_edge_chroma(edge_chroma);
                    let rule = YCbCrVectorscopeRule::new(settings.vectorscope_size, gain)?;
                    // Светлота — из настроек решения: таблица строится здесь, а не на каждом кадре.
                    let palette = Box::new(rule.position_palette(settings.render.vectorscope_lightness));
                    Self::VectorscopeYCbCr(ColouredByPosition { rule, palette })
                }
                VectorscopeAxes::OklabAb => {
                    let range = settings.oklab_range / request.vectorscope_zoom.magnification();
                    let rule = OklabVectorscopeRule::new(settings.vectorscope_size, range, display_gamut)?;
                    let palette = Box::new(rule.position_palette(settings.render.vectorscope_lightness));
                    Self::VectorscopeOklab(ColouredByPosition { rule, palette })
                }
            },
        })
    }

    /// План отсчётов кадра для окна `width × height`: шаги прибора (`ScopeRule::wanted_steps`) под потолок столбцов —
    /// ОДИН вызов для эталона и GPU-стороны.
    pub fn planned_samples(&self, width: u32, height: u32, settings: &ScopeModelSettings) -> SampleCount {
        SampleCount::planned(
            width,
            height,
            ScopeRule::wanted_steps(self, settings),
            settings.trace_max_columns,
        )
    }

    /// Тип прибора.
    pub fn kind(&self) -> ScopeKind {
        match self {
            Self::Waveform(_) => ScopeKind::Waveform,
            Self::LumaWaveform(_) => ScopeKind::LumaWaveform,
            Self::RgbParade(_) => ScopeKind::RgbParade,
            Self::VectorscopeYCbCr(_) | Self::VectorscopeOklab(_) => ScopeKind::Vectorscope,
        }
    }

    /// Как рисуется след этого прибора, вместе с данными способа (цвета, таблица цветов по месту). Единственный
    /// источник способа: эталон ([`crate::render`]) и GPU-часть спрашивают правило, а не угадывают по виду.
    ///
    /// Таблица цветов вектороскопа построена при [`Self::resolve`] (со светлотой из настроек решения), здесь она
    /// только копируется: `settings.vectorscope_lightness` на неё НЕ влияет, цвета секций и палитра наложения
    /// берутся из `settings` как раньше.
    pub fn trace_style(&self, settings: &RenderSettings) -> TraceStyle {
        match self {
            Self::Waveform(_) => TraceStyle::PlanesOverlaid {
                primaries: settings.rgb_waveform_palette.primaries(),
            },
            // Одна плоскость секцией на всю ширину — тем же способом, что parade: новой ветки шейдера нет.
            Self::LumaWaveform(_) => TraceStyle::PlaneSections {
                colors: vec![settings.luma_trace_color],
            },
            Self::RgbParade(_) => TraceStyle::PlaneSections {
                colors: settings.parade_colors.to_vec(),
            },
            Self::VectorscopeYCbCr(coloured) => TraceStyle::ColorByPosition {
                palette: coloured.palette.clone(),
            },
            Self::VectorscopeOklab(coloured) => TraceStyle::ColorByPosition {
                palette: coloured.palette.clone(),
            },
        }
    }

    /// Способ рисования следа (без данных): нужен тем, кто решает по способу, не строя таблицу цветов.
    pub fn trace_paint(&self) -> TracePaint {
        match self {
            Self::Waveform(_) => TracePaint::PlanesOverlaid,
            Self::LumaWaveform(_) | Self::RgbParade(_) => TracePaint::PlaneSections,
            Self::VectorscopeYCbCr(_) | Self::VectorscopeOklab(_) => TracePaint::ColorByPosition,
        }
    }
}

impl ScopeRule for ResolvedScope {
    fn shape(&self, samples: SampleCount) -> GridShape {
        match self {
            Self::Waveform(rule) => rule.shape(samples),
            Self::LumaWaveform(rule) => rule.shape(samples),
            Self::RgbParade(rule) => rule.shape(samples),
            Self::VectorscopeYCbCr(coloured) => coloured.rule.shape(samples),
            Self::VectorscopeOklab(coloured) => coloured.rule.shape(samples),
        }
    }

    fn wanted_steps(&self, settings: &ScopeModelSettings) -> SampleSteps {
        match self {
            // След по столбцам: все отсчёты вдоль взгляда (столбцы сетки), поперёк — реже (решение 329).
            Self::Waveform(_) | Self::LumaWaveform(_) | Self::RgbParade(_) => SampleSteps::for_eye(
                settings.sample_stride,
                settings.trace_across_eye_stride,
                ScopeRule::scan(self),
            ),
            // Вектороскоп собирает весь кадр в один круг: четверти отсчётов хватает (решение 328).
            Self::VectorscopeYCbCr(_) | Self::VectorscopeOklab(_) => {
                SampleSteps::uniform(settings.vectorscope_sample_stride)
            }
        }
    }

    fn scan(&self) -> ScopeScan {
        match self {
            Self::Waveform(rule) => rule.scan(),
            Self::LumaWaveform(rule) => rule.scan(),
            Self::RgbParade(rule) => rule.scan(),
            // Вектороскоп положения не меряет: поле запроса на него не влияет.
            Self::VectorscopeYCbCr(_) | Self::VectorscopeOklab(_) => ScopeScan::Upright,
        }
    }

    fn hits(&self, position: SamplePosition, code: [u16; 3], report: &mut dyn FnMut(Hit)) {
        match self {
            Self::Waveform(rule) => rule.hits(position, code, report),
            Self::LumaWaveform(rule) => rule.hits(position, code, report),
            Self::RgbParade(rule) => rule.hits(position, code, report),
            Self::VectorscopeYCbCr(coloured) => coloured.rule.hits(position, code, report),
            Self::VectorscopeOklab(coloured) => coloured.rule.hits(position, code, report),
        }
    }
}
