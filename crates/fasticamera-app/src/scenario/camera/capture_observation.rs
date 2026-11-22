//! Срезы наблюдения сценария `capture` и сборка записи снимка из журнала запросов и журнала фаз службы.
//!
//! Исполнитель шагов (`capture_runner`, `capture_burst`) — чистый автомат над этими срезами: ему не нужны ни
//! маршрутизатор, ни экран, и его можно гонять в тестах по заранее заготовленным наблюдениям.

use std::collections::HashMap;
use std::time::Instant;

use camera_screen::settings::{AspectRatio, RawCapture};
use camera_screen::shutter::{PrimaryFormat, ShotRequest};
use serde::Serialize;
use shell_contract::clock::MonotonicNanos;
use still_contract::frame_choice::FrameChoice;
use still_contract::plan::StillId;

use super::capture_frame_timing::{ClockBridge, FrameTiming};
use super::capture_report::{ShotOutcome, ShotRecord};
use crate::still_choice_log::ChoiceEntry;
use crate::still_phase_log::{PhaseEntry, PhaseKind};

/// Накопительные счётчики видоискателя, важные для «без пропусков во время серии».
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LiveCounters {
    /// Кадры, которые камера не отдала, потому что пул буферов исчерпан (ошибка расчёта пула).
    pub pool_exhausted: u64,
    /// Кадры, выброшенные переполнением джиттер-буфера (дрейф часов камеры против экрана).
    pub jitter_overflow: u64,
    /// Повторы кадра на экране: заполненный буфер видоискателя оказался пуст.
    pub underruns: u64,
    /// Кадры, показанные позже задуманного vsync (только число, решение 91).
    pub late_frames: u64,
    /// Чёрные кадры там, где ждали картинку.
    pub black_frames_while_picture_expected: u64,
    /// Удержания картинки (на шаге снимка их быть не должно: поток камеры не меняется).
    pub holds_started: u64,
    pub live_frames_presented: u64,
}

impl LiveCounters {
    /// Приращение от `earlier` до `self` (счётчики не убывают; насыщение — защита от перезапуска рендера).
    pub fn delta_since(self, earlier: LiveCounters) -> LiveDelta {
        LiveDelta {
            pool_exhausted: self.pool_exhausted.saturating_sub(earlier.pool_exhausted),
            jitter_overflow: self.jitter_overflow.saturating_sub(earlier.jitter_overflow),
            underruns: self.underruns.saturating_sub(earlier.underruns),
            late_frames: self.late_frames.saturating_sub(earlier.late_frames),
            black_frames_while_picture_expected: self
                .black_frames_while_picture_expected
                .saturating_sub(earlier.black_frames_while_picture_expected),
            holds_started: self.holds_started.saturating_sub(earlier.holds_started),
            live_frames_presented: self.live_frames_presented.saturating_sub(earlier.live_frames_presented),
        }
    }
}

/// Приращения [`LiveCounters`] за окно.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct LiveDelta {
    pub pool_exhausted: u64,
    pub jitter_overflow: u64,
    pub underruns: u64,
    pub late_frames: u64,
    pub black_frames_while_picture_expected: u64,
    pub holds_started: u64,
    pub live_frames_presented: u64,
}

/// Счётчики постановки запросов исполнителем эффектов экрана (накопительные).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SubmitCounts {
    /// Принял сервис снимка.
    pub accepted: u64,
    /// Сервис отверг: уже занят (`Busy`) или недоступен.
    pub rejected: u64,
}

impl SubmitCounts {
    /// Все попытки постановки (экран дошёл до сервиса).
    pub fn submitted(self) -> u64 {
        self.accepted + self.rejected
    }

    pub fn delta_since(self, earlier: SubmitCounts) -> SubmitCounts {
        SubmitCounts {
            accepted: self.accepted.saturating_sub(earlier.accepted),
            rejected: self.rejected.saturating_sub(earlier.rejected),
        }
    }
}

/// Что исполнитель шагов видит на одном тике.
pub struct CaptureObservation<'a> {
    pub now: Instant,
    /// Момент этого тика сценария по `CLOCK_MONOTONIC` — метка нажатия, как `MotionEvent.getEventTime()`
    /// у настоящего касания: по ней рендер ищет кадр, стоявший на экране в момент нажатия.
    ///
    /// Не время последнего показанного кадра: пока рендер занят проявлением, снимок его диагностики
    /// стоит на месте, и несколько нажатий с разницей 100 мс получали одну и ту же старую метку (14c) —
    /// рендер искал кадр, показанный за 100–300 мс до нажатия, и не находил его в кольце.
    pub press_time: MonotonicNanos,
    /// Что экран применит к следующему снимку.
    pub raw_setting: RawCapture,
    /// Всего отказов редьюсера экрана (накопительно). Во время шагов экран получает только нажатия затвора и
    /// ответы службы, поэтому прирост — отказы нажатий (занят затвор, полон набор сохраняемых).
    pub actions_rejected: u64,
    pub submit: SubmitCounts,
    /// Журнал принятых запросов снимка (старые вытеснены).
    pub journal: &'a [ShotRequest],
    pub live: LiveCounters,
    /// Сервис снимка настоящий (иначе прогон теряет смысл).
    pub mode_is_real: bool,
    /// Мост часов `BOOTTIME`↔`MONOTONIC` для перевода метки сенсора в момент нажатия.
    pub clock: ClockBridge,
    /// Времена кадра рендера на этот тик (окно последних кадров).
    pub render_frame_timing: RenderFrameTiming,
}

/// Сводка окна времён кадра рендера, мс (p50/p95/наибольшее по последним кадрам окна диагностики показа).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct TimingSummaryMs {
    pub samples: usize,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
}

impl TimingSummaryMs {
    pub fn from_summary(summary: &render_loop::thread::DurationSummary) -> Self {
        let millis = |duration: std::time::Duration| duration.as_secs_f64() * 1_000.0;
        Self {
            samples: summary.samples,
            p50_ms: millis(summary.p50),
            p95_ms: millis(summary.p95),
            max_ms: millis(summary.max),
        }
    }
}

/// Куда уходит время кадра видоискателя на потоке рендера (окно последних кадров): ждал ли поток GPU
/// (`acquire_wait` — fence слота и картинка swapchain) или стоял в своей работе до сабмита (`latch_to_submit`),
/// и сколько кадр стоил на GPU. Вход 14c: чем вызваны выброшенные кадры в серии.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct RenderFrameTiming {
    pub acquire_wait: Option<TimingSummaryMs>,
    pub latch_to_submit: Option<TimingSummaryMs>,
    pub frame_gpu: Option<TimingSummaryMs>,
}

/// Собранные фазы службы записи за прогон (дополняется порциями журнала).
#[derive(Debug, Default)]
pub struct PhaseBook {
    entries: Vec<PhaseEntry>,
    lost: u64,
    /// Какой кадр выбрал рендер для каждого снимка (по номеру снимка службы).
    choices: HashMap<StillId, FrameChoice>,
}

impl PhaseBook {
    /// Запоминает выборы кадра из журнала форвардера (повтор той же записи безвреден).
    pub fn absorb_choices(&mut self, entries: Vec<ChoiceEntry>) {
        for entry in entries {
            self.choices.insert(entry.still, entry.choice);
        }
    }

    pub fn choice_of(&self, still: StillId) -> Option<FrameChoice> {
        self.choices.get(&still).copied()
    }

    pub fn extend(&mut self, new_entries: Vec<PhaseEntry>, lost: u64) {
        self.entries.extend(new_entries);
        self.lost += lost;
    }

    /// Сколько записей журнала вытеснено до чтения.
    pub fn lost(&self) -> u64 {
        self.lost
    }

    fn of_request(&self, request: u64) -> impl Iterator<Item = &PhaseEntry> {
        self.entries.iter().filter(move |entry| entry.request.0 == request)
    }

    /// Итоговая фаза снимка запроса (`Saved` или `Failed`), если пришла.
    pub fn terminal_of(&self, request: u64) -> Option<&PhaseEntry> {
        self.of_request(request)
            .find(|entry| !matches!(entry.kind, PhaseKind::Secured))
    }

    pub fn secured_of(&self, request: u64) -> Option<&PhaseEntry> {
        self.of_request(request)
            .find(|entry| matches!(entry.kind, PhaseKind::Secured))
    }
}

/// Миллисекунды от `from` до `to` (0, если часы «назад»).
fn millis_between(from: Instant, to: Instant) -> u64 {
    u64::try_from(to.saturating_duration_since(from).as_millis()).unwrap_or(u64::MAX)
}

/// Короткое имя соотношения сторон для отчёта.
pub fn aspect_name(aspect: AspectRatio) -> &'static str {
    match aspect {
        AspectRatio::FourByThree => "4:3",
        AspectRatio::SixteenByNine => "16:9",
        AspectRatio::Square => "1:1",
    }
}

/// Запись снимка по запросу экрана и фазам службы. Времена — от нажатия `pressed_at`.
pub fn shot_record(
    number: u32,
    step: &'static str,
    raw_expected: bool,
    request: &ShotRequest,
    pressed_at: Instant,
    book: &PhaseBook,
    clock: &ClockBridge,
) -> ShotRecord {
    let secured = book.secured_of(request.request.0);
    let terminal = book.terminal_of(request.request.0);
    let outcome = match terminal.map(|entry| &entry.kind) {
        Some(PhaseKind::Saved { primary, raw }) => ShotOutcome::Saved {
            primary: primary.clone(),
            raw: raw.clone(),
        },
        Some(PhaseKind::Failed { failure }) => ShotOutcome::Failed {
            failure: failure.clone(),
        },
        Some(PhaseKind::Secured) | None => ShotOutcome::Unfinished {
            secured: secured.is_some(),
        },
    };
    let still = secured.or(terminal).map(|entry| entry.still);
    let (choice, unknown_why) = match still {
        Some(still) => (
            book.choice_of(still),
            "рендер не сообщил выбор этого снимка (нет захвата) или журнал его вытеснил",
        ),
        None => (None, "служба не назвала номер снимка"),
    };
    let saved_at = terminal.filter(|entry| matches!(entry.kind, PhaseKind::Saved { .. }));
    ShotRecord {
        number,
        step,
        request: request.request.0,
        still: still.map(|id| id.0),
        raw_expected,
        raw_requested: request.raw == RawCapture::On,
        format: match request.primary_format {
            PrimaryFormat::Heif => "heif",
            PrimaryFormat::Jpeg => "jpeg",
        },
        aspect: aspect_name(request.aspect),
        zoom_milli: (request.framing.ratio.value() * 1000.0).round().max(0.0) as u32,
        secured_after_ms: secured.map(|entry| millis_between(pressed_at, entry.at)),
        saved_after_ms: saved_at.map(|entry| millis_between(pressed_at, entry.at)),
        outcome,
        frame: FrameTiming::of(choice, unknown_why, request.pressed_at, clock),
    }
}

#[cfg(test)]
#[path = "capture_observation_tests.rs"]
mod tests;
