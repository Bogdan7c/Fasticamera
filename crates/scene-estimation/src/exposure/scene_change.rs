//! Детектор «сцена под точкой тапа уже другая» (решение 241): чистая арифметика над статистикой кадров.
//!
//! Без часов и потоков: время — только метки сенсора кадров, вызывающий (рендер) подаёт кадры по порядку.
//! Решение «снять ли точку» принимает движок камеры; здесь лишь наблюдение «кадр устойчиво не похож на эталон».
//!
//! # Подпись кадра
//! Плитки статистики сворачиваются в грубую сетку (по умолчанию 8×6). Свет ячейки — **зелёный сырой статистики**
//! (`sum_g`, после виньетирования, до баланса белого), делённый на произведение «выдержка × ISO» ЭТОГО кадра,
//! в `log2`. Поэтому подпись не двигают: смена белой, WARMTH/TINT и веса яркости (зелёный их не знает) и работа AE
//! с компенсацией EV и подтяжкой (делим на произведение кадра). Двигает её только свет сцены.
//!
//! Ячейка знает свой свет по-разному, и сравнение это учитывает (интервал `[нижняя; верхняя]` границы):
//! * измерена — точное значение (`нижняя = верхняя`);
//! * пересвечена — свет не меньше (`[L; +∞)`): белый остаётся белым при любой выдержке;
//! * темна — свет не больше (`(-∞; U]`): ниже шума ничего не видно.
//!
//! Ячейка голосует «изменилась» только когда интервалы кадров разошлись не меньше чем на порог; «не изменилась»
//! она голосует только если обе измерены. Остальные случаи (пересвечена там и там, пересвечена против измеренной выше
//! границы и т. п.) — воздержалась: вывод неоднозначен.
//!
//! # Решение
//! Доля изменившихся среди голосующих ≥ `changed_share` и голосующих не меньше нужного (`min_voting_cells`, но не
//! больше четверти ячеек подписи) — кадр «не похож»; мало голосующих — «вывода нет». «Не похож» должен держаться не
//! меньше `confirm_ms` по меткам сенсора и не меньше `confirm_frames` кадров подряд; кадр «похож» или «без вывода»
//! подтверждение сбрасывает.
//!
//! # Эталон
//! Снимается с ПЕРВОГО годного кадра тапа: подпись от выдержки не зависит, поэтому ждать `Settled` AE незачем, а
//! «тапнул и сразу увёл камеру» иначе снял бы эталон уже с новой сцены.
//!
//! **Уточнение.** Эталон с тёмным или пересвеченным местом знает свет там лишь границей. На кадре, который НЕ голосует
//! «сменилась» (похож или без вывода), такая ячейка эталона становится измеренной, если на кадре она измерена и
//! границе не противоречит (так схождение AE после тапа по тёмному или светлому раскрывает эталон сам и застрять на
//! нём нельзя). Измеренные ячейки не переписываются никогда (иначе медленный уход сцены съел бы эталон), а пока кадр
//! голосует «сменилась», эталон не трогается.
//!
//! **Зум.** Смена кадрирования переснимает эталон с текущего кадра: плитки легли на другой участок сцены. Если в этот
//! момент шло неподтверждённое «не похож», это подтверждение смены, а не повод её забыть.

use camera_contract::auto_exposure::ExposureProduct;
use camera_contract::capture_result::SensorTimestamp;
use camera_contract::geometry::PixelRect;
use camera_contract::three_a::PointRequestId;
use isp_model::statistics::{AwbStatistics, QUANT_SCALE};

use super::scene_change_config::SceneChangeConfig;
use crate::error::ConfigError;

/// Один кадр на входе детектора.
#[derive(Debug, Clone, Copy)]
pub struct SceneFrame<'a> {
    /// Тап, под который следим: другой номер начинает слежение заново.
    pub request: PointRequestId,
    /// Статистика кадра (плитки).
    pub statistics: &'a AwbStatistics,
    /// «Выдержка × ISO» ЭТОГО кадра (из его результата).
    pub product: ExposureProduct,
    /// Метка сенсора кадра.
    pub sensor_timestamp: SensorTimestamp,
    /// Кадрирование, по которому посчитаны плитки: другое — эталон переснимается.
    pub framing: PixelRect,
}

/// Почему снят эталон.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceReason {
    /// Первый годный кадр тапа.
    FirstFrame,
    /// Сменилось кадрирование (зум).
    Reframed,
}

/// Чем подтверждена смена сцены.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChangeEvidence {
    /// Голосующих ячеек, сказавших «изменилась».
    pub changed_cells: u32,
    /// Всего голосующих ячеек.
    pub voting_cells: u32,
}

/// Что сделал детектор с кадром.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneWatchStep {
    /// Кадр не по порядку меток (старее или равен уже виденному): проигнорирован.
    Ignored,
    /// Эталона ещё нет: кадры тапа пока без единой ячейки со светом.
    AwaitingReference,
    /// Эталон снят с этого кадра.
    ReferenceTaken(ReferenceReason),
    /// Кадр похож на эталон.
    Similar,
    /// Голосующих ячеек мало — вывода нет.
    Inconclusive,
    /// Кадр не похож, но подтверждения (время и число кадров) ещё нет.
    Diverging,
    /// Смена сцены подтверждена; пока условие держится, повторяется на каждом кадре.
    Confirmed(ChangeEvidence),
}

/// Свет ячейки грубой сетки: что о нём известно (см. модуль).
#[derive(Debug, Clone, Copy, PartialEq)]
enum CellLight {
    /// Кадр в ячейке пуст (края при неровном делении).
    Unmeasured,
    /// `log2` света измерен.
    Measured(f32),
    /// Свет не меньше: ячейка частично пересвечена.
    AtLeast(f32),
    /// Свет не больше: ячейка темнее шума.
    AtMost(f32),
}

impl CellLight {
    /// Границы света `(нижняя, верхняя)`; `None` — ячейка не знает света вовсе.
    fn bounds(self) -> Option<(f32, f32)> {
        match self {
            CellLight::Unmeasured => None,
            CellLight::Measured(value) => Some((value, value)),
            CellLight::AtLeast(lower) => Some((lower, f32::INFINITY)),
            CellLight::AtMost(upper) => Some((f32::NEG_INFINITY, upper)),
        }
    }

    fn is_measured(self) -> bool {
        matches!(self, CellLight::Measured(_))
    }
}

/// Голос одной ячейки при сравнении с эталоном.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CellVote {
    Changed,
    Same,
    Abstained,
}

/// Подпись кадра: грубая сетка света ячеек.
#[derive(Debug, Clone, PartialEq)]
struct SceneSignature {
    columns: u32,
    rows: u32,
    cells: Vec<CellLight>,
}

/// Суммы ячейки грубой сетки по её плиткам.
#[derive(Debug, Clone, Copy, Default)]
struct CellSums {
    sum_g: u64,
    used_cells: u64,
    saturated_cells: u64,
}

impl SceneSignature {
    /// Подпись статистики `statistics` кадра с произведением `product`.
    fn of(statistics: &AwbStatistics, product: ExposureProduct, config: &SceneChangeConfig) -> Self {
        let grid = statistics.grid();
        let columns = config.grid_columns.min(grid.columns).max(1);
        let rows = config.grid_rows.min(grid.rows).max(1);
        let mut sums = vec![CellSums::default(); (columns * rows) as usize];
        for tile_row in 0..grid.rows {
            for tile_column in 0..grid.columns {
                let Some(tile) = statistics.tile(tile_column, tile_row) else {
                    continue;
                };
                // Плитка попадает в ячейку, чей отрезок сетки её накрывает (целочисленное деление, как у самой статистики).
                let cell_column = u64::from(tile_column) * u64::from(columns) / u64::from(grid.columns);
                let cell_row = u64::from(tile_row) * u64::from(rows) / u64::from(grid.rows);
                let cell = &mut sums[(cell_row * u64::from(columns) + cell_column) as usize];
                cell.sum_g += u64::from(tile.sum_g);
                cell.used_cells += u64::from(tile.used_cells);
                cell.saturated_cells += u64::from(tile.saturated_cells);
            }
        }
        let product_log2 = product.seconds_iso().log2();
        let cells = sums
            .into_iter()
            .map(|cell| cell_light(cell, product_log2, config))
            .collect();
        Self { columns, rows, cells }
    }
}

/// Свет ячейки: измерен, пересвечен (не меньше) или темнее шума (не больше).
///
/// Среднее берётся по ВСЕМ ячейкам статистики, пересвеченные — как 1,0 (нижняя оценка): иначе рост экспозиции,
/// выжигающий отдельные ячейки, сам давал бы ложный голос «стало темнее» (среднее по оставшимся падало бы).
fn cell_light(cell: CellSums, product_log2: f64, config: &SceneChangeConfig) -> CellLight {
    let total = cell.used_cells + cell.saturated_cells;
    if total == 0 {
        return CellLight::Unmeasured;
    }
    // Зелёный даёт два отсчёта на ячейку статистики; масштаб 4095 — доля белого сенсора.
    let green_sum = cell.sum_g as f64 / (2.0 * f64::from(QUANT_SCALE));
    let level = (green_sum + cell.saturated_cells as f64) / total as f64;
    let saturated_share = cell.saturated_cells as f64 / total as f64;
    if saturated_share >= f64::from(config.clipped_cell_share) {
        return CellLight::AtLeast(relative_log2(level, product_log2));
    }
    if level < f64::from(config.dark_green_level) {
        return CellLight::AtMost(relative_log2(f64::from(config.dark_green_level), product_log2));
    }
    CellLight::Measured(relative_log2(level, product_log2))
}

/// `log2(level / product)` — свет, не зависящий от выдержки кадра.
fn relative_log2(level: f64, product_log2: f64) -> f32 {
    (level.log2() - product_log2) as f32
}

/// Как ячейка голосует при сравнении с эталоном (см. модуль).
fn vote(reference: CellLight, current: CellLight, threshold_ev: f32) -> CellVote {
    let (Some((reference_low, reference_high)), Some((current_low, current_high))) =
        (reference.bounds(), current.bounds())
    else {
        return CellVote::Abstained;
    };
    // Интервалы разошлись не меньше чем на порог — вывод однозначен, как бы ни были неточны границы.
    let separated = current_low - reference_high >= threshold_ev || reference_low - current_high >= threshold_ev;
    if separated {
        CellVote::Changed
    } else if reference.is_measured() && current.is_measured() {
        CellVote::Same
    } else {
        CellVote::Abstained
    }
}

/// Итог сравнения кадра с эталоном.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameVerdict {
    Inconclusive,
    Similar,
    Dissimilar(ChangeEvidence),
}

/// Сколько ячеек должно голосовать: `min_voting_cells`, но не больше четверти ячеек подписи (у малой сетки
/// статистики иначе вывода не было бы никогда); не меньше одной.
fn required_voting_cells(signature_cells: usize, config: &SceneChangeConfig) -> u32 {
    let quarter = u32::try_from(signature_cells / 4).unwrap_or(u32::MAX);
    config.min_voting_cells.min(quarter).max(1)
}

/// Сравнение подписей ячейка в ячейку.
fn compare(reference: &SceneSignature, current: &SceneSignature, config: &SceneChangeConfig) -> FrameVerdict {
    let (mut changed_cells, mut voting_cells) = (0_u32, 0_u32);
    for (&before, &now) in reference.cells.iter().zip(&current.cells) {
        match vote(before, now, config.cell_threshold_ev) {
            CellVote::Changed => {
                changed_cells += 1;
                voting_cells += 1;
            }
            CellVote::Same => voting_cells += 1,
            CellVote::Abstained => {}
        }
    }
    if voting_cells < required_voting_cells(current.cells.len(), config) {
        return FrameVerdict::Inconclusive;
    }
    let share = changed_cells as f32 / voting_cells as f32;
    if share >= config.changed_share {
        FrameVerdict::Dissimilar(ChangeEvidence {
            changed_cells,
            voting_cells,
        })
    } else {
        FrameVerdict::Similar
    }
}

/// Непрерывный отрезок «не похож».
#[derive(Debug, Clone, Copy)]
struct DissimilarRun {
    since: SensorTimestamp,
    frames: u32,
    /// Чем подтверждался последний кадр отрезка (его отдаёт зум как подтверждение).
    evidence: ChangeEvidence,
}

/// Эталон снят: слежение за кадрами.
#[derive(Debug, Clone)]
struct Watch {
    reference: SceneSignature,
    dissimilar: Option<DissimilarRun>,
}

/// Слежение за одним тапом.
#[derive(Debug, Clone)]
struct Tracked {
    request: PointRequestId,
    framing: PixelRect,
    last_frame: SensorTimestamp,
    watch: Option<Watch>,
}

/// Детектор смены сцены под точкой тапа (один на сессию показа).
#[derive(Debug, Clone)]
pub struct SceneChangeDetector {
    config: SceneChangeConfig,
    tracked: Option<Tracked>,
}

impl SceneChangeDetector {
    /// Детектор с проверенным конфигом.
    pub fn new(config: SceneChangeConfig) -> Result<Self, ConfigError> {
        config.validate()?;
        Ok(Self { config, tracked: None })
    }

    /// Забыть тап и эталон (новый поток, указание больше не несёт точки).
    pub fn reset(&mut self) {
        self.tracked = None;
    }

    /// Учесть кадр. Кадры подаются в порядке меток сенсора, все одного потока.
    pub fn observe(&mut self, frame: &SceneFrame<'_>) -> SceneWatchStep {
        let signature = SceneSignature::of(frame.statistics, frame.product, &self.config);
        let tracked = match &mut self.tracked {
            Some(tracked) if tracked.request == frame.request => tracked,
            slot => slot.insert(Tracked {
                request: frame.request,
                framing: frame.framing,
                last_frame: SensorTimestamp::from_nanos(i64::MIN),
                watch: None,
            }),
        };
        if frame.sensor_timestamp <= tracked.last_frame {
            return SceneWatchStep::Ignored;
        }
        tracked.last_frame = frame.sensor_timestamp;
        let reframed = tracked.framing != frame.framing
            || tracked.watch.as_ref().is_some_and(|watch| {
                watch.reference.columns != signature.columns || watch.reference.rows != signature.rows
            });
        tracked.framing = frame.framing;
        let Some(watch) = tracked.watch.as_mut() else {
            // Первый годный кадр тапа (хоть одна ячейка со светом) — эталон.
            if signature.is_blank() {
                return SceneWatchStep::AwaitingReference;
            }
            tracked.watch = Some(Watch::new(signature));
            return SceneWatchStep::ReferenceTaken(ReferenceReason::FirstFrame);
        };
        if reframed {
            // Неподтверждённое «не похож» в момент зума — подтверждение смены, а не повод её забыть.
            let pending_change = watch.dissimilar.map(|run| run.evidence);
            *watch = Watch::new(signature);
            return match pending_change {
                Some(evidence) => SceneWatchStep::Confirmed(evidence),
                None => SceneWatchStep::ReferenceTaken(ReferenceReason::Reframed),
            };
        }
        watch.step(frame.sensor_timestamp, &signature, &self.config)
    }
}

impl Watch {
    fn new(reference: SceneSignature) -> Self {
        Self {
            reference,
            dissimilar: None,
        }
    }

    /// Сравнить кадр с эталоном, обновить подтверждение и (если кадр не голосует «сменилась») уточнить эталон.
    fn step(&mut self, now: SensorTimestamp, signature: &SceneSignature, config: &SceneChangeConfig) -> SceneWatchStep {
        match compare(&self.reference, signature, config) {
            FrameVerdict::Similar => {
                self.dissimilar = None;
                self.reference.refine_with(signature, config.cell_threshold_ev);
                SceneWatchStep::Similar
            }
            FrameVerdict::Inconclusive => {
                self.dissimilar = None;
                self.reference.refine_with(signature, config.cell_threshold_ev);
                SceneWatchStep::Inconclusive
            }
            FrameVerdict::Dissimilar(evidence) => {
                // Эталон не трогаем: пока кадр голосует «сменилась», уточнять его нечем и незачем.
                let run = self.dissimilar.get_or_insert(DissimilarRun {
                    since: now,
                    frames: 0,
                    evidence,
                });
                run.frames += 1;
                run.evidence = evidence;
                let confirmed =
                    run.frames >= config.confirm_frames && elapsed_at_least(run.since, now, config.confirm_ms);
                if confirmed {
                    SceneWatchStep::Confirmed(evidence)
                } else {
                    SceneWatchStep::Diverging
                }
            }
        }
    }
}

impl SceneSignature {
    /// Нет ни одной ячейки со светом.
    fn is_blank(&self) -> bool {
        self.cells.iter().all(|cell| *cell == CellLight::Unmeasured)
    }

    /// Уточнение эталона (см. модуль): ячейка-граница становится измеренной, если на кадре она измерена и границе не
    /// противоречит. Измеренные ячейки не переписываются.
    fn refine_with(&mut self, current: &SceneSignature, threshold_ev: f32) {
        for (reference, &now) in self.cells.iter_mut().zip(&current.cells) {
            let is_bound = matches!(reference, CellLight::AtLeast(_) | CellLight::AtMost(_));
            if is_bound && now.is_measured() && vote(*reference, now, threshold_ev) != CellVote::Changed {
                *reference = now;
            }
        }
    }
}

/// Прошло ли от `since` до `now` не меньше `milliseconds` по меткам сенсора.
fn elapsed_at_least(since: SensorTimestamp, now: SensorTimestamp, milliseconds: u32) -> bool {
    let elapsed_nanos = now.as_nanos().saturating_sub(since.as_nanos());
    elapsed_nanos >= i64::from(milliseconds) * 1_000_000
}

#[cfg(test)]
#[path = "scene_change_tests.rs"]
mod tests;
