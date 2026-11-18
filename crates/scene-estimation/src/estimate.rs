//! Результаты оценки: сырой выход редукции («что вернул бы шейдер») и итог после скалярного шага CPU.

use isp_model::color::chromaticity::Chromaticity;
use isp_model::color::white_point::{CctDuv, LocusClamp};

/// Сырой результат редукции плиток: несколько f32, которые compute-шейдер записал бы в буфер.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawEstimate {
    /// Средний майред принятых плиток (0, если принятых нет).
    pub mired: f32,
    /// Средний Duv принятых плиток.
    pub duv: f32,
    /// Доля веса принятых плиток во всём весе пригодных плиток, `[0, 1]`.
    pub confidence: f32,
    /// Сколько плиток принято как «серые».
    pub accepted_tiles: u32,
    /// Сколько плиток пригодны для оценки вообще (достаточно непересвеченных ячеек, нормальная освещённость).
    pub eligible_tiles: u32,
}

impl RawEstimate {
    /// Принятых плиток нет — майред и Duv не определены.
    pub fn has_candidates(&self) -> bool {
        self.accepted_tiles > 0
    }
}

/// Оценка света сцены после скалярного шага CPU: белая точка, уверенность, отчёт о прижиме к полосе.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IlluminantEstimate {
    /// Цветность белой (после привязки к полосе вокруг кривой Планка).
    pub white: Chromaticity,
    /// То же в виде «температура + Duv».
    pub cct_duv: CctDuv,
    /// Уверенность `[0, 1]`: доля веса пригодных плиток, принятых как серые.
    pub confidence: f32,
    /// Сколько плиток принято как «серые».
    pub accepted_tiles: u32,
    /// Что пришлось прижать при привязке к полосе (до прижима — в отчёте).
    pub clamped: LocusClamp,
    /// Сырой выход редукции.
    pub raw: RawEstimate,
}

/// Исход оценки одного кадра статистики.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EstimateOutcome {
    /// Есть хотя бы одна серая плитка: оценка с уверенностью (решать, годится ли она, будет контроллер).
    Estimated(IlluminantEstimate),
    /// Серых плиток нет вовсе (сцена без нейтральных или слишком тёмная/пересвеченная).
    NoCandidates {
        /// Сколько плиток было пригодно для оценки.
        eligible_tiles: u32,
    },
}
