//! Статистика пейсинга: насколько ровно кадры доходят до экрана.
//!
//! Чистая логика без часов: на вход — моменты реального показа кадров (`MonotonicNanos`)
//! вместе с номером показа (`present_id`), на выход — гистограмма интервалов в периодах
//! обновления, доля пропусков и вердикт.
//!
//! Определения:
//! * интервал — разность моментов показа двух СОСЕДНИХ номеров (`id` и `id + 1`).
//!   Между несоседними номерами интервала нет: драйвер хранит лишь короткую историю
//!   времён показа (Android: `MAX_TIMING_INFOS = 10` на swapchain в `libvulkan/swapchain.cpp`), и записи
//!   между редкими опросами теряются. Разность через потерянные записи — не пропуск
//!   кадра, а дыра в измерении;
//! * дыра (`timing_gaps`) — сколько записей потеряно между соседними в окне по номерам.
//!   Дыры считаются отдельно и в долю пропусков не входят;
//! * пропуск (miss) — интервал между соседними номерами строго больше 1,5 периода;
//! * окно — последние `window` времени по меткам; старые метки вытесняются;
//! * разрыв непрерывности (пересоздание цепочки): номера новой цепочки не продолжают
//!   старые, поэтому пара через разрыв не даёт ни интервала, ни дыры;
//! * смена периода обновления (экран переключил 60 -> 120 Гц) очищает окно: интервалы
//!   разных периодов нельзя смешивать в одной гистограмме.

use std::collections::VecDeque;
use std::time::Duration;

use shell_contract::clock::MonotonicNanos;

use crate::config::RenderLoopConfig;

/// Число корзин гистограммы. Корзина `k` — интервал, округлённый до `k` периодов;
/// последняя собирает всё, что длиннее.
pub const HISTOGRAM_BUCKETS: usize = 8;

/// Порог пропуска в долях периода, выраженный дробью 3/2 (целочисленная арифметика
/// без ошибок округления на границе).
const MISS_THRESHOLD_NUMERATOR: i128 = 3;
const MISS_THRESHOLD_DENOMINATOR: i128 = 2;

/// Для вердикта нужно, чтобы наблюдаемый отрезок покрывал не меньше этой доли окна (в %):
/// на коротком отрезке одна случайная задержка даёт ложный провал.
const MIN_WINDOW_FILL_PERCENT: i128 = 90;

/// Параметры статистики.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PacingParams {
    /// Ожидаемый период обновления экрана.
    pub refresh_period: Duration,
    /// Допустимая доля пропусков, 0..=1.
    pub max_miss_ratio: f64,
    /// Длина скользящего окна.
    pub window: Duration,
    /// Минимальная относительная разница периодов, при которой новый период считается сменой
    /// режима экрана (см. `RenderLoopConfig::refresh_period_change_ratio`). Меньшая разница —
    /// шум измерения: установленный период и окно остаются.
    pub period_change_ratio: f64,
    /// Минимальная доля кадров окна, давших честный интервал (`intervals / frames_covered`);
    /// ниже — вердикт `SparseData` (см. `RenderLoopConfig::pacing_min_coverage_ratio`).
    pub min_coverage_ratio: f64,
}

impl PacingParams {
    /// Берёт параметры из конфигурации потока.
    pub fn from_config(config: &RenderLoopConfig) -> Self {
        Self {
            refresh_period: config.expected_refresh_period(),
            max_miss_ratio: config.pacing_max_miss_ratio,
            window: config.pacing_window(),
            period_change_ratio: config.refresh_period_change_ratio,
            min_coverage_ratio: config.pacing_min_coverage_ratio,
        }
    }
}

/// Вердикт по окну.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PacingVerdict {
    /// Доля пропусков в пределах нормы.
    Passed,
    /// Пропусков больше допустимого.
    Failed {
        miss_ratio: f64,
        max_miss_ratio: f64,
        misses: u64,
        intervals: u64,
    },
    /// Окно ещё не набралось (или нет ни одного интервала): судить рано.
    NotEnoughData,
    /// Окно набралось по времени, но интервалов в нём слишком мало относительно кадров: много
    /// потерянных записей драйвера или разрывов цепочки. Доля пропусков по горстке интервалов
    /// ничего не доказывает, поэтому это не `Passed`, а отдельная причина отказа.
    SparseData {
        /// `intervals / frames_covered`.
        coverage_ratio: f64,
        /// Порог из конфигурации.
        min_coverage_ratio: f64,
        intervals: u64,
        frames_covered: u64,
    },
}

/// Снимок статистики.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PacingSnapshot {
    /// Сколько интервалов в окне.
    pub intervals: u64,
    /// Гистограмма: корзина `k` — интервалы, округлённые до `k` периодов.
    pub histogram: [u64; HISTOGRAM_BUCKETS],
    /// Интервалы длиннее 1,5 периода.
    pub misses: u64,
    /// `misses / intervals` (0 при пустом окне).
    pub miss_ratio: f64,
    /// Какой отрезок времени реально покрывает окно (от первой до последней метки).
    pub observed_span: Duration,
    /// Потерянные записи времён показа между соседними по номеру метками окна. Это дыры
    /// измерения, а не пропуски кадров: в `misses` и `miss_ratio` они не входят.
    pub timing_gaps: u64,
    /// Сколько кадров охватывает окно: метки плюс потерянные между ними записи.
    /// `intervals / frames_covered` — доля кадров, реально попавших в статистику.
    pub frames_covered: u64,
    /// Сколько меток отброшено из-за обратного хода часов (время показа меньше предыдущего).
    /// Ненулевое число — повод не доверять статистике; в отчёт идёт, чтобы это было видно.
    pub rejected_out_of_order: u64,
    pub verdict: PacingVerdict,
}

impl PacingSnapshot {
    /// Пустой снимок «данных нет».
    pub fn empty() -> Self {
        Self {
            intervals: 0,
            histogram: [0; HISTOGRAM_BUCKETS],
            misses: 0,
            miss_ratio: 0.0,
            observed_span: Duration::ZERO,
            timing_gaps: 0,
            frames_covered: 0,
            rejected_out_of_order: 0,
            verdict: PacingVerdict::NotEnoughData,
        }
    }
}

/// Одна метка показа окна.
#[derive(Debug, Clone, Copy)]
struct PresentMark {
    /// Номер показа в цепочке бэкенда (растёт на 1 за показ, циклический `u32`).
    present_id: u32,
    /// Эпоха непрерывности: меняется на разрыве (пересоздание цепочки).
    epoch: u64,
    actual_nanos: i128,
}

/// Накопитель меток показа со скользящим окном.
#[derive(Debug)]
pub struct PacingTracker {
    params: PacingParams,
    period_nanos: i128,
    window_nanos: i128,
    /// Метки показа по возрастанию времени.
    presents: VecDeque<PresentMark>,
    /// Текущая эпоха непрерывности; пары меток разных эпох не образуют интервала.
    epoch: u64,
    /// Метки с неубывающим порядком нарушены (часы пошли назад) — такие игнорируются.
    rejected_out_of_order: u64,
    /// Сколько раз смена периода очищала окно.
    period_changes: u64,
}

impl PacingTracker {
    pub fn new(params: PacingParams) -> Self {
        Self {
            params,
            period_nanos: duration_nanos(params.refresh_period).max(1),
            window_nanos: duration_nanos(params.window),
            presents: VecDeque::new(),
            epoch: 0,
            rejected_out_of_order: 0,
            period_changes: 0,
        }
    }

    /// Меняет период обновления, по которому судится статистика (реальный период экрана,
    /// ставший известным после старта или изменившийся на ходу: 60 -> 120 Гц).
    /// Окно ОЧИЩАЕТСЯ: интервалы старого и нового периодов нельзя смешивать (гистограмма
    /// и порог пропуска привязаны к одному периоду). Выбран сброс, а не корзины по периодам:
    /// после смены режима важна картина на новом периоде, а окно набирается заново.
    /// Нулевой период отвергается (деление на ноль в корзинах). Изменение не больше
    /// `period_change_ratio` от установленного периода — дрожание измерения драйвера, а не
    /// смена режима: период и окно остаются нетронутыми (иначе окно очищалось бы каждым
    /// опросом и не копилось). Возвращает `true`, только если период действительно сменился.
    pub fn set_refresh_period(&mut self, period: Duration) -> bool {
        let period_nanos = duration_nanos(period);
        if period_nanos <= 0 || !self.is_real_period_change(period_nanos) {
            return false;
        }
        self.period_nanos = period_nanos;
        self.params.refresh_period = period;
        self.presents.clear();
        self.period_changes += 1;
        true
    }

    /// Отличается ли `candidate_nanos` от установленного периода больше чем на порог.
    fn is_real_period_change(&self, candidate_nanos: i128) -> bool {
        let difference = (candidate_nanos - self.period_nanos).unsigned_abs();
        // f64 достаточно: периоды — миллионы наносекунд, порог — доля процента.
        (difference as f64) > (self.period_nanos as f64) * self.params.period_change_ratio
    }

    /// Сообщает о разрыве непрерывности (цепочку показа пересоздали или присоединили заново):
    /// номера показов новой цепочки не продолжают старые, и пара меток через разрыв
    /// не должна давать ни интервала, ни дыры.
    pub fn mark_discontinuity(&mut self) {
        self.epoch += 1;
    }

    /// Сколько раз смена периода очищала окно.
    pub fn period_changes(&self) -> u64 {
        self.period_changes
    }

    /// Период обновления, по которому сейчас считается статистика.
    pub fn refresh_period(&self) -> Duration {
        self.params.refresh_period
    }

    /// Сколько меток отброшено из-за нарушения порядка.
    pub fn rejected_out_of_order(&self) -> u64 {
        self.rejected_out_of_order
    }

    /// Добавляет момент реального показа кадра с номером показа `present_id`.
    pub fn record_present(&mut self, present_id: u32, actual_present: MonotonicNanos) {
        let nanos = i128::from(actual_present.as_nanos());
        if self.presents.back().is_some_and(|last| nanos < last.actual_nanos) {
            self.rejected_out_of_order += 1;
            return;
        }
        self.presents.push_back(PresentMark {
            present_id,
            epoch: self.epoch,
            actual_nanos: nanos,
        });
        // Вытесняем метки старше окна, но всегда оставляем две последние: иначе интервала нет.
        while self.presents.len() > 2
            && self
                .presents
                .front()
                .is_some_and(|first| nanos - first.actual_nanos > self.window_nanos)
        {
            self.presents.pop_front();
        }
    }

    /// Считает статистику по текущему окну.
    pub fn snapshot(&self) -> PacingSnapshot {
        let mut histogram = [0u64; HISTOGRAM_BUCKETS];
        let mut misses = 0u64;
        let mut intervals = 0u64;
        let mut timing_gaps = 0u64;
        for (earlier, later) in self.presents.iter().zip(self.presents.iter().skip(1)) {
            match classify_pair(earlier, later) {
                PairKind::Consecutive => {
                    let interval = later.actual_nanos - earlier.actual_nanos;
                    histogram[self.bucket_of(interval)] += 1;
                    if interval * MISS_THRESHOLD_DENOMINATOR > self.period_nanos * MISS_THRESHOLD_NUMERATOR {
                        misses += 1;
                    }
                    intervals += 1;
                }
                PairKind::LostRecords(lost) => timing_gaps += lost,
                PairKind::Unrelated => {}
            }
        }
        let span_nanos = match (self.presents.front(), self.presents.back()) {
            (Some(first), Some(last)) => last.actual_nanos - first.actual_nanos,
            _ => 0,
        };
        let miss_ratio = if intervals == 0 {
            0.0
        } else {
            misses as f64 / intervals as f64
        };
        let frames_covered = self.presents.len() as u64 + timing_gaps;
        PacingSnapshot {
            intervals,
            histogram,
            misses,
            miss_ratio,
            observed_span: Duration::from_nanos(u64::try_from(span_nanos).unwrap_or(0)),
            timing_gaps,
            frames_covered,
            rejected_out_of_order: self.rejected_out_of_order,
            verdict: self.verdict(intervals, frames_covered, misses, miss_ratio, span_nanos),
        }
    }

    fn verdict(
        &self,
        intervals: u64,
        frames_covered: u64,
        misses: u64,
        miss_ratio: f64,
        span_nanos: i128,
    ) -> PacingVerdict {
        let window_filled = span_nanos * 100 >= self.window_nanos * MIN_WINDOW_FILL_PERCENT;
        // frames_covered >= 1 при intervals >= 1 (метки есть), деление безопасно.
        let coverage_ratio = intervals as f64 / frames_covered.max(1) as f64;
        if intervals == 0 || !window_filled {
            PacingVerdict::NotEnoughData
        } else if coverage_ratio < self.params.min_coverage_ratio {
            PacingVerdict::SparseData {
                coverage_ratio,
                min_coverage_ratio: self.params.min_coverage_ratio,
                intervals,
                frames_covered,
            }
        } else if miss_ratio <= self.params.max_miss_ratio {
            PacingVerdict::Passed
        } else {
            PacingVerdict::Failed {
                miss_ratio,
                max_miss_ratio: self.params.max_miss_ratio,
                misses,
                intervals,
            }
        }
    }

    /// Номер корзины: интервал в периодах, округлённый к ближайшему целому, не выше последней.
    fn bucket_of(&self, interval_nanos: i128) -> usize {
        let rounded_periods = (interval_nanos * 2 + self.period_nanos) / (self.period_nanos * 2);
        usize::try_from(rounded_periods).map_or(HISTOGRAM_BUCKETS - 1, |bucket| bucket.min(HISTOGRAM_BUCKETS - 1))
    }
}

/// Как две соседние по времени метки связаны по номерам показа.
enum PairKind {
    /// Номера соседние (`id`, `id + 1`) в одной эпохе: между ними есть честный интервал.
    Consecutive,
    /// Номера в одной эпохе, но между ними потеряны записи (число потерянных).
    LostRecords(u64),
    /// Другая эпоха, повтор или номер назад: связи нет, ни интервала, ни дыры.
    Unrelated,
}

/// Номер «вперёд» дальше этой границы считается откатом назад (циклический `u32`).
const MAX_FORWARD_ID_STEP: u32 = 1 << 31;

fn classify_pair(earlier: &PresentMark, later: &PresentMark) -> PairKind {
    if earlier.epoch != later.epoch {
        return PairKind::Unrelated;
    }
    match later.present_id.wrapping_sub(earlier.present_id) {
        1 => PairKind::Consecutive,
        step @ 2..MAX_FORWARD_ID_STEP => PairKind::LostRecords(u64::from(step - 1)),
        _ => PairKind::Unrelated,
    }
}

fn duration_nanos(duration: Duration) -> i128 {
    i128::try_from(duration.as_nanos()).unwrap_or(i128::MAX)
}

#[cfg(test)]
#[path = "pacing_tests.rs"]
mod tests;
