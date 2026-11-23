//! Шов нового прибора: правило попадания и общее накопление.
//!
//! Новый прибор (гистограмма, «ложный цвет») = новая реализация [`ScopeRule`]; [`accumulate`] и
//! счётчики не меняются. Тип прибора — данные (форма сетки + функция попадания), а не ветвление.

use crate::codes::ShownCodes;
use crate::grid::{GridShape, ScopeCounters};
use crate::sampling::SampleStride;

/// Одно попадание: ячейка сетки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    /// Плоскость (канал у parade).
    pub plane: u32,
    /// Строка (0 — верх картинки прибора).
    pub row: u32,
    /// Столбец.
    pub column: u32,
}

/// Где стоит отсчёт на показанной картинке. Правилам waveform и parade нужен столбец картинки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SamplePosition {
    /// Столбец отсчёта на показанной картинке (с учётом зеркала).
    pub x: u32,
    /// Ширина показанной картинки.
    pub picture_width: u32,
}

/// Правило попадания прибора.
pub trait ScopeRule {
    /// Форма сетки счётчиков.
    fn shape(&self) -> GridShape;

    /// Сообщает попадания (0…N) одного отсчёта. Ячейка вне `shape()` — ошибка правила: [`accumulate`]
    /// вернёт её, а не проглотит. «Нет попадания» (вектороскоп вне круга) — ноль вызовов `report`.
    fn hits(&self, position: SamplePosition, code: [u16; 3], report: &mut dyn FnMut(Hit));
}

/// Ошибка накопления.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AccumulateError {
    /// Правило сообщило ячейку вне своей сетки.
    #[error("правило сообщило ячейку вне сетки: плоскость {}, строка {}, столбец {}", .0.plane, .0.row, .0.column)]
    HitOutsideGrid(Hit),
}

/// Накопление счётчиков: проход по отсчётам `stride` строго в порядке строк.
pub fn accumulate<R: ScopeRule + ?Sized>(
    picture: &ShownCodes,
    stride: SampleStride,
    rule: &R,
) -> Result<ScopeCounters, AccumulateError> {
    let mut counters = ScopeCounters::zeroed(rule.shape());
    let mut violation: Option<Hit> = None;
    for row_index in 0..stride.samples_along(picture.height()) {
        let y = stride.coordinate(row_index);
        for column_index in 0..stride.samples_along(picture.width()) {
            let x = stride.coordinate(column_index);
            let Some(code) = picture.pixel(x, y) else { continue };
            let position = SamplePosition {
                x,
                picture_width: picture.width(),
            };
            rule.hits(position, code, &mut |hit| {
                if !counters.add_hit(hit.plane, hit.row, hit.column) && violation.is_none() {
                    violation = Some(hit);
                }
            });
        }
        if let Some(hit) = violation {
            return Err(AccumulateError::HitOutsideGrid(hit));
        }
    }
    Ok(counters)
}

/// Попадания одного кода: для графики прибора (куда ляжет цвет) и тестов. Идёт через то же правило.
pub fn hits_of_code<R: ScopeRule + ?Sized>(rule: &R, position: SamplePosition, code: [u16; 3]) -> Vec<Hit> {
    let mut hits = Vec::new();
    rule.hits(position, code, &mut |hit| hits.push(hit));
    hits
}
