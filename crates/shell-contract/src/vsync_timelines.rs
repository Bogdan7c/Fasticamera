//! Все линии кадра одного vsync (сессия 07): не только предпочитаемая, но и соседние.
//!
//! Choreographer (API 33) на каждый vsync отдаёт СПИСОК возможных моментов показа — «линий
//! кадра». У каждой свой срок сдачи (`deadline`): к нему кадр должен быть готов, чтобы
//! появиться на экране в `expected_present`. Платформа помечает одну линию как
//! предпочитаемую (по ней она «запланировала» приложение), но приложение вправе выбрать
//! другую: например, самую раннюю, к сроку которой оно успевает. Это и есть путь к меньшей
//! задержке «сенсор → экран» для видоискателя (наследство сессий 03 и 05).
//!
//! Тик vsync — `Copy` и публикуется на каждом vsync, поэтому линии лежат в массиве
//! фиксированной ёмкости [`MAX_FRAME_TIMELINES`] без выделения памяти. Если платформа прислала
//! больше линий, лишние отбрасываются ЯВНО: их число видно в [`FrameTimelines::dropped_count`]
//! (потребитель считает такие тики), а не теряется молча.

use crate::clock::MonotonicNanos;

/// Ёмкость списка линий в тике.
///
/// В AOSP число линий ограничено `VsyncEventData::kFrameTimelinesCapacity` (7 в Android 13–14);
/// берём 8 — с запасом в одну линию. Больше линий видоискателю и не нужно: дальние линии —
/// это показ через 100+ мс, задержка, которой мы как раз избегаем.
pub const MAX_FRAME_TIMELINES: usize = 8;

/// Одна линия кадра: когда кадр будет показан и к какому сроку он должен быть готов.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VsyncTimeline {
    /// Идентификатор линии в SurfaceFlinger (для отчётов о показе).
    pub vsync_id: i64,
    /// Когда кадр этой линии будет показан на экране.
    pub expected_present: MonotonicNanos,
    /// Срок, к которому кадр должен быть готов, чтобы попасть в `expected_present`.
    pub deadline: MonotonicNanos,
}

impl VsyncTimeline {
    /// Заполнитель неиспользуемых ячеек массива (наружу никогда не выдаётся).
    const UNUSED: Self = Self {
        vsync_id: 0,
        expected_present: MonotonicNanos::from_nanos(0),
        deadline: MonotonicNanos::from_nanos(0),
    };
}

/// Почему список линий не собран. Варианты различимы: «пусто» и «индекс мимо» —
/// разные сбои платформы.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FrameTimelinesError {
    #[error("платформа не передала ни одной линии кадра")]
    NoTimelines,
    #[error("предпочитаемая линия {preferred_index} вне списка из {timeline_count} линий")]
    PreferredIndexOutOfRange {
        preferred_index: usize,
        timeline_count: usize,
    },
}

/// Линии кадра одного vsync в порядке платформы + индекс предпочитаемой.
///
/// Инварианты (держит конструктор, поля закрыты): список не пуст, `preferred_index` внутри
/// списка, длина не больше [`MAX_FRAME_TIMELINES`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameTimelines {
    entries: [VsyncTimeline; MAX_FRAME_TIMELINES],
    len: usize,
    preferred_index: usize,
    /// Сколько линий платформы не поместилось в ёмкость и отброшено.
    dropped: usize,
}

impl FrameTimelines {
    /// Список из одной линии — она же предпочитаемая (источник, который знает только её; тесты).
    pub fn only(preferred: VsyncTimeline) -> Self {
        let mut entries = [VsyncTimeline::UNUSED; MAX_FRAME_TIMELINES];
        entries[0] = preferred;
        Self {
            entries,
            len: 1,
            preferred_index: 0,
            dropped: 0,
        }
    }

    /// Собирает список из `timeline_count` линий платформы; `read(index)` вызывается только
    /// для `index < timeline_count` и только для сохраняемых линий.
    ///
    /// Если линий больше ёмкости, сохраняется непрерывное окно из [`MAX_FRAME_TIMELINES`] линий,
    /// в котором обязательно есть предпочитаемая: `[0, ёмкость)`, а если предпочитаемая дальше —
    /// окно, кончающееся на ней. Отбрасываются дальние линии (показ позже нужного) либо, во
    /// втором случае, ещё и самые ранние; их число — [`Self::dropped_count`].
    pub fn from_platform_list(
        timeline_count: usize,
        preferred_index: usize,
        mut read: impl FnMut(usize) -> VsyncTimeline,
    ) -> Result<Self, FrameTimelinesError> {
        if timeline_count == 0 {
            return Err(FrameTimelinesError::NoTimelines);
        }
        if preferred_index >= timeline_count {
            return Err(FrameTimelinesError::PreferredIndexOutOfRange {
                preferred_index,
                timeline_count,
            });
        }
        let kept = timeline_count.min(MAX_FRAME_TIMELINES);
        // Начало окна: 0, если предпочитаемая помещается, иначе окно заканчивается на ней.
        let first_kept = (preferred_index + 1).saturating_sub(MAX_FRAME_TIMELINES);
        let mut entries = [VsyncTimeline::UNUSED; MAX_FRAME_TIMELINES];
        for (slot, platform_index) in entries.iter_mut().zip(first_kept..first_kept + kept) {
            *slot = read(platform_index);
        }
        Ok(Self {
            entries,
            len: kept,
            preferred_index: preferred_index - first_kept,
            dropped: timeline_count - kept,
        })
    }

    /// Сохранённые линии в порядке платформы (не пусто).
    pub fn as_slice(&self) -> &[VsyncTimeline] {
        &self.entries[..self.len]
    }

    /// Индекс предпочитаемой линии в [`Self::as_slice`] (не в исходном списке платформы,
    /// если ранние линии были отброшены).
    pub fn preferred_index(&self) -> usize {
        self.preferred_index
    }

    /// Предпочитаемая линия платформы.
    pub fn preferred(&self) -> VsyncTimeline {
        self.entries[self.preferred_index]
    }

    /// Сколько линий платформы не поместилось в ёмкость (0 — список полный).
    pub fn dropped_count(&self) -> usize {
        self.dropped
    }
}
