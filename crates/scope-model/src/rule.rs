//! Шов нового прибора: правило попадания и общее накопление.
//!
//! Новый прибор (гистограмма, «ложный цвет») = новая реализация [`ScopeRule`]; [`accumulate`] и
//! счётчики не меняются. Тип прибора — данные (форма сетки + функция попадания), а не ветвление.

use scope_contract::ScopeScan;

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

/// Где стоит отсчёт НА ГОРИЗОНТАЛИ ГЛАЗА: правилам waveform и parade нужна не колонка поверхности, а
/// положение вдоль оси сцены «слева направо» (см. [`ScopeScan`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SamplePosition {
    /// Координата отсчёта вдоль взгляда «слева направо» (с учётом зеркала картинки и поворота телефона).
    pub x: u32,
    /// Длина этой оси на показанной картинке (ширина или высота поверхности — по повороту).
    pub picture_width: u32,
}

impl SamplePosition {
    /// Положение отсчёта `(x, y)` ПОКАЗАННОЙ картинки `width × height` для того, кто читает её по `scan`.
    ///
    /// Показанная картинка уже зеркальна, если зеркален экран; поворот лишь описывает, как глаз её читает,
    /// и накладывается поверх (зеркало → чтение).
    pub fn from_shown(x: u32, y: u32, width: u32, height: u32, scan: ScopeScan) -> Self {
        let (along, extent) = if scan.runs_along_surface_rows() {
            (y, height)
        } else {
            (x, width)
        };
        let along = if scan.is_reversed() { extent - 1 - along } else { along };
        Self {
            x: along,
            picture_width: extent,
        }
    }
}

/// Правило попадания прибора.
pub trait ScopeRule {
    /// Форма сетки счётчиков.
    fn shape(&self) -> GridShape;

    /// Как глаз читает показанную картинку слева направо. Прибору, которому положение не важно
    /// (вектороскоп), оставляется чтение по умолчанию — результат от него не зависит.
    fn scan(&self) -> ScopeScan {
        ScopeScan::Upright
    }

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
            let position = SamplePosition::from_shown(x, y, picture.width(), picture.height(), rule.scan());
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
