//! Словарь выбора кадра из кольца снимков (решение 139): запрос, причины замены, исходы.
//!
//! crate намеренно НЕ зависит от `still-contract`: здесь свои перечисления, а рендер переводит
//! их в `still_contract::frame_choice`. Логика выбора — в `stream/ring_pick.rs` (под замком хаба).

use camera_contract::capture_result::SensorTimestamp;

use crate::lease::RawFrameLease;

/// Какой кадр кольца нужен снимку.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingPickRequest {
    /// Кадр, который рендер реально показал на экране в момент нажатия: известна его точная метка.
    ShownFrame(SensorTimestamp),
    /// Рендер точного кадра не знает: нужен ближайший к оценке момента показа.
    NearestToEstimate(SensorTimestamp),
}

impl RingPickRequest {
    /// Метка, к которой тянется выбор (точная или оценочная).
    pub fn target(self) -> SensorTimestamp {
        match self {
            Self::ShownFrame(target) | Self::NearestToEstimate(target) => target,
        }
    }
}

/// Почему вместо запрошенного кадра выдан другой.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingSubstitutionReason {
    /// Цель старше старейшего кадра кольца: её уже вытеснили.
    TargetEvicted,
    /// Цель в кольце есть, но у неё нет метаданных (`Missing`): без паспорта кадр не годится.
    TargetWithoutMeta,
    /// Цель в кольце есть, но это кадр серии, фокус-триггера, разовой вспышки или неопознанный
    /// запрос, а не обычный кадр повторяющегося запроса.
    TargetNotRepeating,
    /// Цели нет в кольце, хотя она не старше старейшего кадра (потерян, не дошёл до кольца).
    TargetNotInRing,
    /// Точной цели не было с самого начала: запрос `NearestToEstimate`.
    EstimateRequested,
}

/// Почему кадр выбрать нельзя вообще.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingUnavailableReason {
    /// Кольцо выключено (ёмкость 0): бюджет нулевой или поток не начат.
    RingDisabled,
    /// Кольцо включено, но кадров в нём ещё нет.
    RingEmpty,
    /// Кадры есть, но ни один не годится (все без метаданных или не из повторяющегося запроса).
    NoEligibleFrame,
}

/// Итог [`SnapshotRingFrames::pick`](crate::SnapshotRingFrames::pick).
///
/// Лизинг в `Exact`/`Substituted` — отдельная доля: потребитель обязан вернуть её
/// (`release_unused` / `release_after_gpu_fence`); кольцо сохраняет свою долю.
#[derive(Debug)]
#[must_use = "выданную долю кадра надо вернуть явно"]
pub enum RingPick {
    /// Запрошенный кадр найден и годен.
    Exact(RawFrameLease),
    /// Выдан ближайший годный кадр вместо запрошенного.
    Substituted {
        lease: RawFrameLease,
        reason: RingSubstitutionReason,
        /// Метка, которую просили (точная или оценочная).
        requested: SensorTimestamp,
    },
    /// Выдать нечего.
    Unavailable(RingUnavailableReason),
}

/// Итог [`SnapshotRingFrames::pick_flash_frame`](crate::SnapshotRingFrames::pick_flash_frame):
/// поиск кадра разового запроса со вспышкой (14d) ТОЛЬКО по метке запроса.
///
/// Намеренно отдельный от [`RingPick`]: здесь нет «замены» — другой кадр вместо кадра со вспышкой
/// не годится никогда (это был бы обычный кадр без вспышки под видом снимка со вспышкой).
#[derive(Debug)]
#[must_use = "выданную долю кадра надо вернуть явно"]
pub enum FlashRingPick {
    /// Кадр запроса `FlashStill { shot }` с метаданными найден; доля отдельная, кольцо свою хранит.
    Found(RawFrameLease),
    /// Такого кадра в кольце пока нет: он ещё не дошёл (или ждёт свой результат). Спросить позже.
    NotInRingYet,
    /// Кольцо выключено (ёмкость 0): кадр не появится никогда.
    RingDisabled,
}
