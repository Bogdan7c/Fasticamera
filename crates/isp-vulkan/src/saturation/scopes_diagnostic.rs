//! Диагностические варианты накопления приборов для стенда насыщения (НЕ продукт).
//!
//! Цель — разложить цену прохода накопления на части (чтение картинки, проекция, запись `atomicAdd`, очистка
//! счётчиков), не правя продуктовый шейдер: каждый вариант — отдельный шейдер, зарегистрированный через шов
//! `ColorScopes::with_extra_accumulators`, а прибор подменяет только идентификатор шейдера ([`DiagnosticRule`]).
//! Счётчики в таких вариантах НЕ равны эталону (запись пропущена или перенаправлена): отпечаток выхода у них
//! осмысленно отличается от продуктового.

use scope_contract::ScopeScan;
use scope_model::{GraticulePrimitive, GridShape, PositionPalette, RenderSettings, ResolvedScope, TracePaint};

use crate::scopes::{AccumulateParameters, AccumulatorId, AccumulatorShader, GpuScopeRule, ScopeError};
use crate::shaders::blobs;

/// Какой кусок накопления оставлен (целочисленные приборы: waveform, parade, вектороскоп Y'CbCr).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccumulateDiagnostic {
    /// Пустой шейдер: остаётся очистка счётчиков, запуск групп и барьеры.
    Empty,
    /// Чтение показанной картинки и квантование кода, без проекции и записи.
    ReadOnly,
    /// Чтение + проекция, без записи счётчиков.
    SkipWrite,
    /// Полное накопление, но все `atomicAdd` в одну ячейку.
    WriteCellZero,
    /// Полное накопление, но у каждой нити своя ячейка (без конкуренции).
    WriteSpread,
    /// Полное накопление со слиянием одинаковых ячеек внутри subgroup (счётчики должны совпасть с продуктом).
    Aggregated,
    /// Нить обходит 4 отсчёта по y и сливает серии одинаковых ячеек в регистре.
    Runs4,
    /// То же, 8 отсчётов.
    Runs8,
    /// То же, 16 отсчётов.
    Runs16,
}

impl AccumulateDiagnostic {
    /// Идентификатор шейдера варианта.
    pub const fn id(self) -> AccumulatorId {
        match self {
            Self::Empty => AccumulatorId::new("diag-empty"),
            Self::ReadOnly => AccumulatorId::new("diag-read-only"),
            Self::SkipWrite => AccumulatorId::new("diag-skip-write"),
            Self::WriteCellZero => AccumulatorId::new("diag-write-cell-zero"),
            Self::WriteSpread => AccumulatorId::new("diag-write-spread"),
            Self::Aggregated => AccumulatorId::new("diag-aggregated"),
            Self::Runs4 => AccumulatorId::new("diag-runs4"),
            Self::Runs8 => AccumulatorId::new("diag-runs8"),
            Self::Runs16 => AccumulatorId::new("diag-runs16"),
        }
    }

    /// Шейдер варианта для регистрации.
    pub fn shader(self) -> AccumulatorShader {
        let spirv = match self {
            Self::Empty => blobs::SCOPE_DIAG_EMPTY_COMP,
            Self::ReadOnly => blobs::SCOPE_DIAG_READ_ONLY_COMP,
            Self::SkipWrite => blobs::SCOPE_DIAG_INTEGER_SKIP_WRITE_COMP,
            Self::WriteCellZero => blobs::SCOPE_DIAG_INTEGER_CELL_ZERO_COMP,
            Self::WriteSpread => blobs::SCOPE_DIAG_INTEGER_SPREAD_COMP,
            Self::Aggregated => blobs::SCOPE_DIAG_INTEGER_AGGREGATE_COMP,
            Self::Runs4 => blobs::SCOPE_DIAG_INTEGER_RUNS4_COMP,
            Self::Runs8 => blobs::SCOPE_DIAG_INTEGER_RUNS8_COMP,
            Self::Runs16 => blobs::SCOPE_DIAG_INTEGER_RUNS16_COMP,
        };
        let rows_per_thread = match self {
            Self::Runs4 => 4,
            Self::Runs8 => 8,
            Self::Runs16 => 16,
            _ => 1,
        };
        AccumulatorShader {
            id: self.id(),
            spirv,
            rows_per_thread,
        }
    }
}

/// Прибор `scope-model`, у которого подменён шейдер накопления; всё остальное — от настоящего прибора.
pub(super) struct DiagnosticRule<'a> {
    pub(super) inner: &'a ResolvedScope,
    pub(super) diagnostic: AccumulateDiagnostic,
}

impl GpuScopeRule for DiagnosticRule<'_> {
    fn accumulator(&self) -> AccumulatorId {
        self.diagnostic.id()
    }

    fn grid(&self) -> GridShape {
        self.inner.grid()
    }

    fn scan(&self) -> ScopeScan {
        GpuScopeRule::scan(self.inner)
    }

    fn accumulate_parameters(&self) -> Result<AccumulateParameters<'_>, ScopeError> {
        self.inner.accumulate_parameters()
    }

    fn trace_colors(&self, settings: &RenderSettings) -> Vec<[f32; 3]> {
        self.inner.trace_colors(settings)
    }

    fn trace_paint(&self) -> TracePaint {
        GpuScopeRule::trace_paint(self.inner)
    }

    fn position_palette(&self, settings: &RenderSettings) -> Option<PositionPalette> {
        GpuScopeRule::position_palette(self.inner, settings)
    }

    fn graticule(&self, settings: &RenderSettings) -> Vec<GraticulePrimitive> {
        self.inner.graticule(settings)
    }
}
