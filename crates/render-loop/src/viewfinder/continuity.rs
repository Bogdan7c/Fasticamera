//! Диагностика непрерывности картинки при смене объектива (сессия 13): метки кадров, счётчики удержаний
//! и кадрирование каждого кадра.
//!
//! Это нейтральные типы (без Vulkan): их заполняет `vulkan_backend::camera::continuity` и сессия показа,
//! читают сценарии, smoke и тесты. Задел V7: будущей записи нужен непрерывный ряд кадров, поэтому КАЖДЫЙ
//! показанный кадр размечен — живой или переходный (`FrameLabel`), а чёрный кадр, записанный там, где ждали
//! картинку камеры, виден отдельным счётчиком (`black_frames_while_picture_expected` обязан быть 0).

use std::collections::VecDeque;

use camera_contract::geometry::PixelRect;
use camera_contract::ids::CameraId;
use camera_contract::zoom::FramedCrop;
use shell_contract::clock::MonotonicNanos;
use ui_draw::frame_source::BackdropContinuity;

/// Почему началось удержание.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldReason {
    /// Поток закрылся (опрос видоискателя ответил `StreamInactive`).
    StreamInactive,
    /// Кадр нового потока (или тот же поток) без известного цвета: AWB ещё скрывает кадр.
    ColorUnknown,
    /// Кадр нового потока, цвет известен, но для яркости нет цели (ни замера, ни объявления движка).
    ExposureWithoutTarget,
    /// Кадр нового потока, цвет известен, цель яркости есть, но уровень кадра дальше допуска.
    ExposureLevelOff,
}

/// Как кончилось удержание.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldEnd {
    /// Новый поток готов (цвет известен и яркость правдоподобна).
    Condition,
    /// Вышло `max_hold_ms`: новый кадр показан как есть.
    Timeout,
    /// Потока нет `max_hold_without_stream_ms`: честный чёрный.
    NoStream,
    /// Поток шлёт кадры, но показать нечего дольше `max_hold_ms` (постоянный отказ ISP): честный чёрный.
    NoPresentableFrame,
}

/// Почему кадр чёрный.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlackKind {
    /// Картинки камеры ещё не было (холодный старт): допустимо.
    NoPicture,
    /// Картинка камеры была и ожидалась — недопустимо: счётчик обязан оставаться нулём.
    PictureExpected,
    /// Камеры нет дольше `max_hold_without_stream_ms`: честное состояние, не сбой.
    CameraUnavailable,
    /// Камера стримит, но показать кадр не удаётся дольше `max_hold_ms` (отказ ISP): честное состояние, не сбой.
    PictureUnavailable,
}

/// Вид перехода.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionKind {
    /// Удержан последний показанный кадр.
    HeldLastFrame,
}

/// Метка кадра: живой, переходный или чёрный.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameLabel {
    /// Живой кадр.
    Live,
    /// Переходный кадр.
    Transition(TransitionKind),
    /// Чёрный кадр.
    Black(BlackKind),
}

/// Кадрирование одного обработанного кадра: чей кадр, какой прямоугольник RAW взят и что показал зум.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameCropReport {
    /// Камера кадра (по паспорту его потока): ей и принадлежит кадрирование.
    pub camera: CameraId,
    /// Прямоугольник буфера RAW, который обработал ISP.
    pub resolved: PixelRect,
    /// Итог формулы кадрирования (центр, зеркало, показанный коэффициент).
    pub framed: FramedCrop,
}

/// Запись об одном показанном кадре.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameRecord {
    /// Время показа кадра (шкала vsync).
    pub present_time: MonotonicNanos,
    /// Метка кадра по факту записи.
    pub label: FrameLabel,
    /// Что рендер сообщил UI о фоне в этом кадре.
    pub backdrop: BackdropContinuity,
    /// Кадрирование нового кадра, записанного в ISP в этом показе (`None` — нового кадра не было).
    pub crop: Option<FrameCropReport>,
    /// Увеличение удержанного кадра вокруг центра в этом показе (1 — без увеличения; у живого кадра всегда 1).
    pub held_magnification: f32,
}

/// Счётчики непрерывности (накопительные за жизнь бэкенда).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContinuityCounters {
    /// Сколько удержаний началось.
    pub holds_started: u64,
    /// Из них — потому что поток закрылся (`StreamInactive`).
    pub holds_started_stream_inactive: u64,
    /// Из них — кадр без известного цвета.
    pub holds_started_color_unknown: u64,
    /// Из них — у яркости нового потока не было цели (ни замера, ни объявления движка для этой камеры).
    pub holds_started_exposure_without_target: u64,
    /// Из них — цель яркости была, но уровень кадра дальше допуска.
    pub holds_started_exposure_level_off: u64,
    /// Сколько кадров показано удержанными (переходными).
    pub held_frames_presented: u64,
    /// Сколько кадров показано живыми.
    pub live_frames_presented: u64,
    /// Сколько удержаний закончилось по условию (цвет и яркость).
    pub holds_revealed_by_condition: u64,
    /// Сколько удержаний закончилось по сроку `max_hold_ms`.
    pub holds_revealed_by_timeout: u64,
    /// Сколько удержаний закончилось честным чёрным (потока не было `max_hold_without_stream_ms`).
    pub holds_ended_without_stream: u64,
    /// Сколько удержаний закончилось честным чёрным, потому что поток шлёт кадры, а показать нечего.
    pub holds_ended_without_presentable_frame: u64,
    /// Длительность последнего законченного удержания, мс.
    pub last_hold_duration_ms: Option<u64>,
    /// Чёрные кадры там, где ждали картинку камеры (смена потока). Должно быть 0.
    pub black_frames_while_picture_expected: u64,
    /// Чёрные кадры до самой первой картинки (холодный старт): допустимо.
    pub black_frames_before_first_picture: u64,
    /// Чёрные кадры «камера недоступна» после честного чёрного: допустимо.
    pub black_frames_camera_unavailable: u64,
    /// Чёрные кадры «камера стримит, показать нечего» после честного чёрного: допустимо.
    pub black_frames_picture_unavailable: u64,
    /// Сколько раз удержание началось без картинки у ISP (нечего держать): кадры чёрные.
    pub holds_without_image: u64,
    /// Кадры нового потока, обработанные ISP, но замененные на экране удержанным кадром.
    pub frames_hidden_by_continuity: u64,
}

/// Диагностика непрерывности в `ViewfinderDiagnostics`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ContinuityDiagnostics {
    /// Счётчики.
    pub counters: ContinuityCounters,
    /// Идёт ли удержание прямо сейчас.
    pub holding: bool,
    /// Последние показанные кадры (старые — первыми), не больше `viewfinder.continuity.recent_frames`.
    pub recent_frames: Vec<FrameRecord>,
}

/// Накопитель непрерывности за жизнь бэкенда (владелец — `ViewfinderStats`).
#[derive(Debug, Default)]
pub struct ContinuityStats {
    counters: ContinuityCounters,
    recent: VecDeque<FrameRecord>,
}

impl ContinuityStats {
    /// Счётчики для правки владельцем показа.
    pub fn counters_mut(&mut self) -> &mut ContinuityCounters {
        &mut self.counters
    }

    /// Запоминает показанный кадр и учитывает его метку. `capacity` — сколько последних кадров помнить (0 — ни одного).
    pub fn note_frame(&mut self, record: FrameRecord, capacity: usize) {
        match record.label {
            FrameLabel::Live => self.counters.live_frames_presented += 1,
            FrameLabel::Transition(TransitionKind::HeldLastFrame) => self.counters.held_frames_presented += 1,
            FrameLabel::Black(BlackKind::NoPicture) => self.counters.black_frames_before_first_picture += 1,
            FrameLabel::Black(BlackKind::PictureExpected) => self.counters.black_frames_while_picture_expected += 1,
            FrameLabel::Black(BlackKind::CameraUnavailable) => self.counters.black_frames_camera_unavailable += 1,
            FrameLabel::Black(BlackKind::PictureUnavailable) => self.counters.black_frames_picture_unavailable += 1,
        }
        if capacity == 0 {
            return;
        }
        while self.recent.len() >= capacity {
            self.recent.pop_front();
        }
        self.recent.push_back(record);
    }

    /// Учитывает начало удержания и его причину.
    pub fn note_hold_started(&mut self, reason: HoldReason) {
        self.counters.holds_started += 1;
        let by_reason = match reason {
            HoldReason::StreamInactive => &mut self.counters.holds_started_stream_inactive,
            HoldReason::ColorUnknown => &mut self.counters.holds_started_color_unknown,
            HoldReason::ExposureWithoutTarget => &mut self.counters.holds_started_exposure_without_target,
            HoldReason::ExposureLevelOff => &mut self.counters.holds_started_exposure_level_off,
        };
        *by_reason += 1;
    }

    /// Учитывает конец удержания.
    pub fn note_hold_ended(&mut self, end: HoldEnd, duration_ms: u64) {
        match end {
            HoldEnd::Condition => self.counters.holds_revealed_by_condition += 1,
            HoldEnd::Timeout => self.counters.holds_revealed_by_timeout += 1,
            HoldEnd::NoStream => self.counters.holds_ended_without_stream += 1,
            HoldEnd::NoPresentableFrame => self.counters.holds_ended_without_presentable_frame += 1,
        }
        self.counters.last_hold_duration_ms = Some(duration_ms);
    }

    /// Снимок для диагностики; `holding` знает сессия показа (машина непрерывности).
    pub fn snapshot(&self, holding: bool) -> ContinuityDiagnostics {
        ContinuityDiagnostics {
            counters: self.counters.clone(),
            holding,
            recent_frames: self.recent.iter().cloned().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(label: FrameLabel, at_ms: i64) -> FrameRecord {
        FrameRecord {
            present_time: MonotonicNanos::from_nanos(at_ms * 1_000_000),
            label,
            backdrop: BackdropContinuity::Live,
            crop: None,
            held_magnification: 1.0,
        }
    }

    #[test]
    fn labels_go_to_their_own_counters_and_the_ring_keeps_only_the_latest() {
        let mut stats = ContinuityStats::default();
        let labels = [
            FrameLabel::Black(BlackKind::NoPicture),
            FrameLabel::Live,
            FrameLabel::Transition(TransitionKind::HeldLastFrame),
            FrameLabel::Black(BlackKind::PictureExpected),
            FrameLabel::Black(BlackKind::CameraUnavailable),
            FrameLabel::Black(BlackKind::PictureUnavailable),
            FrameLabel::Live,
        ];
        for (index, label) in labels.into_iter().enumerate() {
            stats.note_frame(record(label, index as i64), 4);
        }
        let diagnostics = stats.snapshot(true);
        let counters = &diagnostics.counters;
        assert_eq!(
            (
                counters.black_frames_before_first_picture,
                counters.live_frames_presented,
                counters.held_frames_presented,
                counters.black_frames_while_picture_expected,
                counters.black_frames_camera_unavailable,
                counters.black_frames_picture_unavailable,
            ),
            (1, 2, 1, 1, 1, 1)
        );
        assert!(diagnostics.holding);
        let kept: Vec<_> = diagnostics.recent_frames.iter().map(|frame| frame.label).collect();
        assert_eq!(kept, labels[3..], "кольцо на 4 записи держит последние четыре кадра");
    }

    #[test]
    fn hold_endings_are_counted_per_kind_and_remember_the_last_duration() {
        let mut stats = ContinuityStats::default();
        stats.note_hold_started(HoldReason::ExposureLevelOff);
        stats.note_hold_ended(HoldEnd::Condition, 120);
        stats.note_hold_ended(HoldEnd::Timeout, 1500);
        stats.note_hold_ended(HoldEnd::NoPresentableFrame, 1600);
        stats.note_hold_ended(HoldEnd::NoStream, 3000);
        let counters = stats.snapshot(false).counters;
        assert_eq!(
            (
                counters.holds_started,
                counters.holds_revealed_by_condition,
                counters.holds_revealed_by_timeout,
                counters.holds_ended_without_stream,
                counters.holds_ended_without_presentable_frame,
                counters.last_hold_duration_ms,
            ),
            (1, 1, 1, 1, 1, Some(3000))
        );
    }

    #[test]
    fn each_hold_start_is_counted_under_its_reason() {
        let mut stats = ContinuityStats::default();
        for reason in [
            HoldReason::StreamInactive,
            HoldReason::ColorUnknown,
            HoldReason::ExposureWithoutTarget,
            HoldReason::ExposureWithoutTarget,
            HoldReason::ExposureLevelOff,
        ] {
            stats.note_hold_started(reason);
        }
        let counters = stats.snapshot(false).counters;
        assert_eq!(
            (
                counters.holds_started,
                counters.holds_started_stream_inactive,
                counters.holds_started_color_unknown,
                counters.holds_started_exposure_without_target,
                counters.holds_started_exposure_level_off,
            ),
            (5, 1, 1, 2, 1)
        );
    }

    #[test]
    fn zero_capacity_counts_frames_but_keeps_no_history() {
        let mut stats = ContinuityStats::default();
        stats.note_frame(record(FrameLabel::Live, 0), 0);
        let diagnostics = stats.snapshot(false);
        assert_eq!(diagnostics.counters.live_frames_presented, 1);
        assert!(diagnostics.recent_frames.is_empty());
    }
}
