//! Состояние одного снимка внутри службы: билет допуска, срок и открытые ресурсы.

use std::time::Instant;

use shell_contract::still_media::PendingMedia;
use still_contract::shot::StillFrameFacts;

use super::super::admission::AdmissionTicket;
use super::super::stats::StillCounters;
use super::encoder_queue::EncoderSeat;
use crate::exif::{CameraDescription, CaptureMoment};

/// Черновик файла в хранилище.
pub(super) type Draft = Box<dyn PendingMedia>;

/// Снимок в работе. Билет живёт ровно столько же, сколько запись о снимке: его уничтожение —
/// единственное место, где освобождается слот допуска.
pub(super) struct Job {
    pub(super) ticket: AdmissionTicket,
    pub(super) stage: Stage,
}

pub(super) enum Stage {
    /// Допуск выдан, кадра от рендера ещё нет.
    AwaitingCapture { deadline: Instant },
    /// Кадр захвачен: черновики открыты (кодер — когда найдётся место), ждём итог проявления.
    Developing(Box<Developing>),
}

impl Stage {
    pub(super) fn deadline(&self) -> Instant {
        match self {
            Self::AwaitingCapture { deadline } => *deadline,
            Self::Developing(developing) => developing.deadline,
        }
    }
}

/// Ресурсы снимка после `Secured` (кодер HEIF может ещё ждать свободного места).
pub(super) struct Developing {
    pub(super) deadline: Instant,
    pub(super) facts: Box<StillFrameFacts>,
    pub(super) camera: CameraDescription,
    pub(super) moment: CaptureMoment,
    /// Черновик основной картинки.
    pub(super) primary: Draft,
    /// Черновик DNG, если он в плане.
    pub(super) raw: Option<Draft>,
    /// Место снимка под кодер HEIF (для JPEG кодера нет).
    pub(super) encoder: EncoderSeat,
}

impl Developing {
    /// Бросает всё: кодер прерывается, черновики удаляются (частичный файл в галерее не появится).
    pub(super) fn release_everything(self, counters: &StillCounters) {
        let Self {
            encoder, primary, raw, ..
        } = self;
        // Ждущий кодера снимок ничего не открывал; открытый кодер прерывается и освобождает место.
        if let EncoderSeat::Open(encoder) = encoder {
            encoder.abort();
        }
        discard_draft(counters, primary);
        if let Some(raw) = raw {
            discard_draft(counters, raw);
        }
    }
}

/// Удаляет черновик и считает удаление.
pub(super) fn discard_draft(counters: &StillCounters, draft: Draft) {
    counters.record_discarded_draft();
    draft.discard();
}
