//! Публикация статистики AWB видоискателя: «последняя побеждает» (latest-wins).
//!
//! Статистику считает ISP в том же проходе, что и видоискатель; поток рендера забирает её со слота
//! после fence (никогда не ждёт GPU) и публикует сюда вместе с тем, ЧЕЙ это кадр: номер в потоке,
//! метка сенсора (`BOOTTIME`) и номер кадра Camera2. Будущий владелец AWB (сессия 08) читает
//! [`AwbStatisticsReader::latest`] когда удобно; пропуск промежуточных публикаций — норма
//! (AWB сходится за десятки кадров, ему нужна свежая статистика, а не каждая).
//!
//! Замок короткий: публикация — замена `Arc`, чтение — клон `Arc`. Сама статистика не копируется.

use std::sync::{Arc, Mutex, PoisonError};

use isp_model::statistics::AwbStatistics;

/// Порядковый номер публикации (растёт на 1 с каждой; первая — 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AwbPublication(pub u64);

/// Чей кадр посчитан.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AwbFrameIdentity {
    /// Порядковый номер кадра в нашем потоке (`FrameSequence`).
    pub sequence: u64,
    /// Метка сенсора, наносекунды шкалы `BOOTTIME`.
    pub sensor_timestamp_nanos: i64,
    /// Номер кадра Camera2 (`None` — у кадра не было метаданных).
    pub frame_number: Option<i64>,
}

/// Опубликованная статистика.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedAwbStatistics {
    /// Номер публикации.
    pub publication: AwbPublication,
    /// Кадр, по которому посчитано.
    pub frame: AwbFrameIdentity,
    /// Суммы по плиткам (раскладка — `isp_model::statistics`).
    pub statistics: AwbStatistics,
}

#[derive(Default)]
struct FeedState {
    latest: Option<Arc<PublishedAwbStatistics>>,
    published: u64,
}

/// Сторона публикации (держит бэкенд показа; копия — у сессии показа, лента одна).
#[derive(Clone)]
pub struct AwbStatisticsFeed {
    state: Arc<Mutex<FeedState>>,
}

impl std::fmt::Debug for AwbStatisticsFeed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AwbStatisticsFeed")
    }
}

impl AwbStatisticsFeed {
    /// Пустая лента: публикаций не было.
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(FeedState::default())),
        }
    }

    /// Читатель ленты (сколько угодно; дёшево клонируется).
    pub fn reader(&self) -> AwbStatisticsReader {
        AwbStatisticsReader {
            state: Arc::clone(&self.state),
        }
    }

    /// Публикует статистику кадра, вытесняя прежнюю. Возвращает номер публикации.
    pub(crate) fn publish(&self, frame: AwbFrameIdentity, statistics: AwbStatistics) -> AwbPublication {
        let mut state = lock(&self.state);
        state.published += 1;
        let publication = AwbPublication(state.published);
        state.latest = Some(Arc::new(PublishedAwbStatistics {
            publication,
            frame,
            statistics,
        }));
        publication
    }
}

impl Default for AwbStatisticsFeed {
    fn default() -> Self {
        Self::new()
    }
}

/// Сторона чтения (для composition root и будущего AWB).
#[derive(Clone)]
pub struct AwbStatisticsReader {
    state: Arc<Mutex<FeedState>>,
}

impl std::fmt::Debug for AwbStatisticsReader {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AwbStatisticsReader")
    }
}

impl AwbStatisticsReader {
    /// Последняя опубликованная статистика (`None` — публикаций не было).
    pub fn latest(&self) -> Option<Arc<PublishedAwbStatistics>> {
        lock(&self.state).latest.clone()
    }

    /// Последняя статистика, если она новее `seen` (иначе `None`): читателю не надо сравнивать самому.
    pub fn newer_than(&self, seen: Option<AwbPublication>) -> Option<Arc<PublishedAwbStatistics>> {
        self.latest()
            .filter(|published| seen.is_none_or(|seen| published.publication > seen))
    }

    /// Сколько публикаций было за всё время.
    pub fn publications(&self) -> u64 {
        lock(&self.state).published
    }
}

/// Замок ленты: состояние — одно значение, паника держателя его не портит, отказывать читателю незачем.
fn lock(state: &Mutex<FeedState>) -> std::sync::MutexGuard<'_, FeedState> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use isp_model::statistics::{AwbTile, TileGrid};

    use super::*;

    fn statistics(red_sum: u32) -> AwbStatistics {
        let grid = TileGrid { columns: 1, rows: 1 };
        let tile = AwbTile {
            sum_r: red_sum,
            sum_g: 2,
            sum_b: 3,
            used_cells: 4,
            saturated_cells: 0,
        };
        AwbStatistics::from_tiles(grid, vec![tile]).expect("одна плитка на сетку 1×1")
    }

    fn frame(sequence: u64) -> AwbFrameIdentity {
        AwbFrameIdentity {
            sequence,
            sensor_timestamp_nanos: 1_000 + i64::try_from(sequence).unwrap_or(0),
            frame_number: Some(7),
        }
    }

    #[test]
    fn reader_sees_only_the_latest_publication_with_its_frame() {
        let feed = AwbStatisticsFeed::new();
        let reader = feed.reader();
        assert_eq!(reader.latest(), None, "до публикаций — пусто");
        assert_eq!(feed.publish(frame(1), statistics(1)), AwbPublication(1));
        assert_eq!(feed.publish(frame(2), statistics(5)), AwbPublication(2));
        let latest = reader.latest().expect("есть публикация");
        assert_eq!(latest.publication, AwbPublication(2));
        assert_eq!(latest.frame, frame(2), "статистика привязана к своему кадру");
        assert_eq!(latest.statistics, statistics(5));
        assert_eq!(reader.publications(), 2);
    }

    #[test]
    fn newer_than_filters_already_seen_publications() {
        let feed = AwbStatisticsFeed::new();
        let reader = feed.reader();
        assert_eq!(reader.newer_than(None), None);
        feed.publish(frame(1), statistics(1));
        let first = reader.newer_than(None).expect("новая публикация").publication;
        assert_eq!(reader.newer_than(Some(first)), None, "уже видели");
        feed.publish(frame(2), statistics(1));
        assert_eq!(
            reader.newer_than(Some(first)).map(|published| published.frame.sequence),
            Some(2)
        );
    }
}
