//! Шов нового прибора: правило попадания и общее накопление.
//!
//! Новый прибор (гистограмма, «ложный цвет») = новая реализация [`ScopeRule`]; [`accumulate`] и
//! счётчики не меняются. Тип прибора — данные (форма сетки + функция попадания), а не ветвление.

use scope_contract::ScopeScan;

use crate::codes::ShownCodes;
use crate::grid::{GridShape, ScopeCounters};
use crate::sample_count::SampleCount;
use crate::sampling::SampleSteps;
use crate::settings::ScopeModelSettings;

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
/// номер отсчёта вдоль оси сцены «слева направо» (см. [`ScopeScan`]). Столбец сетки следа — ровно этот номер
/// (решение 322): у каждого измеренного столбца кадра свой столбец счёта, пустых и двойных столбцов нет.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SamplePosition {
    /// Номер отсчёта вдоль взгляда «слева направо» (с учётом зеркала картинки и поворота телефона), `0..eye_samples`.
    pub eye_index: u32,
    /// Сколько отсчётов вдоль этой оси ([`SampleCount::along_eye`]).
    pub eye_samples: u32,
}

impl SamplePosition {
    /// Положение отсчёта с номерами `(column_index, row_index)` (по осям ПОКАЗАННОЙ картинки) для того, кто читает
    /// её по `scan`.
    ///
    /// Показанная картинка уже зеркальна, если зеркален экран; поворот лишь описывает, как глаз её читает,
    /// и накладывается поверх (зеркало → чтение). Отсчёты вдоль оси стоят через равный шаг, поэтому «читать с
    /// конца» — это просто номер с конца.
    pub fn of_sample(column_index: u32, row_index: u32, samples: SampleCount, scan: ScopeScan) -> Self {
        let along = if scan.runs_along_surface_rows() {
            row_index
        } else {
            column_index
        };
        let eye_samples = samples.along_eye(scan);
        let eye_index = if scan.is_reversed() {
            eye_samples - 1 - along
        } else {
            along
        };
        Self { eye_index, eye_samples }
    }
}

/// Правило попадания прибора.
pub trait ScopeRule {
    /// Форма сетки счётчиков для кадра с отсчётами `samples`. Waveform и parade берут из них число столбцов
    /// (столбец = номер отсчёта вдоль взгляда); приборам, чья сетка от кадра не зависит (вектороскоп), отсчёты
    /// безразличны.
    fn shape(&self, samples: SampleCount) -> GridShape;

    /// Шаги прореживания по осям, которые прибор просит у настроек (до плана под потолок столбцов,
    /// [`SampleCount::planned`]). По умолчанию — общий `sample_stride` по обеим осям; след по столбцам просит все
    /// отсчёты вдоль взгляда и реже поперёк (решение 329), вектороскоп — свой шаг (решение 328).
    fn wanted_steps(&self, settings: &ScopeModelSettings) -> SampleSteps {
        SampleSteps::uniform(settings.sample_stride)
    }

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

/// Накопление счётчиков: проход по отсчётам с шагами `steps` (один шаг — на обе оси) строго в порядке строк. Шаги
/// берутся как есть: продукт заранее выбирает их планом [`SampleCount::planned`] (потолок столбцов сетки следа).
pub fn accumulate<R: ScopeRule + ?Sized>(
    picture: &ShownCodes,
    steps: impl Into<SampleSteps>,
    rule: &R,
) -> Result<ScopeCounters, AccumulateError> {
    let steps = steps.into();
    let samples = SampleCount::for_window(picture.width(), picture.height(), steps);
    let mut counters = ScopeCounters::zeroed(rule.shape(samples));
    let mut violation: Option<Hit> = None;
    for row_index in 0..samples.down() {
        let y = steps.down().coordinate(row_index);
        for column_index in 0..samples.across() {
            let x = steps.across().coordinate(column_index);
            let Some(code) = picture.pixel(x, y) else { continue };
            let position = SamplePosition::of_sample(column_index, row_index, samples, rule.scan());
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
