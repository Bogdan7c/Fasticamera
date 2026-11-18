//! Приём предложений рендера: отсев предложений прошлого потока.
//!
//! Метки кадров (`SENSOR_TIMESTAMP`) одной шкалы `BOOTTIME` и растут, поэтому предложение
//! по кадру **раньше** первого результата нового потока заведомо посчитано по картинке
//! прошлого потока (другая камера, другая экспозиция) и применять его нельзя.
//!
//! Пока первого результата нового потока нет, метки отсчёта тоже нет. Выбираем безопаснее
//! всего: предложение **придерживается**, а не применяется и не выбрасывается. Выбросить
//! вслепую нельзя: рендер публикует только при изменении цели, и честное первое предложение,
//! обогнавшее результат на другом потоке, при стабильной сцене не повторилось бы. Когда метка
//! появилась — придержанное предложение либо принимается (`≥ метки`), либо считается устаревшим.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use camera_contract::auto_exposure::ExposureProposal;
use camera_contract::capture_result::SensorTimestamp;

/// Что решил отсев о придержанном предложении.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum GateVerdict {
    /// Придерживать нечего.
    NothingToReview,
    /// Предложение есть, но метки нового потока ещё нет — ждём первый результат.
    Waiting,
    /// Предложение отброшено как относящееся к прошлому потоку (счётчик уже увеличен).
    RejectedStale,
    /// Предложение принято.
    Accepted(ExposureProposal),
}

#[derive(Debug, Default)]
pub(super) struct ProposalGate {
    /// Метка первого результата повторяющегося запроса текущего потока.
    stream_start: Option<SensorTimestamp>,
    /// Самое свежее предложение, ещё не прошедшее отсев (последнее побеждает).
    held: Option<ExposureProposal>,
    /// Общий счётчик: исполнитель живёт в движке в `Box<dyn ..>`, а читать счётчик хочется
    /// снаружи (диагностика композиции, тесты).
    stale_rejected: Arc<AtomicU64>,
}

impl ProposalGate {
    /// Принять предложение на проверку; прежнее непроверенное заменяется.
    pub(super) fn offer(&mut self, proposal: ExposureProposal) {
        self.held = Some(proposal);
    }

    /// Первый результат повторяющегося запроса нового потока. Повторные вызовы метку не
    /// сдвигают. `true` — метка появилась только что и есть придержанное предложение:
    /// исполнителю пора заглянуть в `review` (толкнуть себя).
    pub(super) fn mark_stream_start(&mut self, first_frame: SensorTimestamp) -> bool {
        if self.stream_start.is_some() {
            return false;
        }
        self.stream_start = Some(first_frame);
        self.held.is_some()
    }

    /// Проверить придержанное предложение.
    pub(super) fn review(&mut self) -> GateVerdict {
        let Some(proposal) = self.held else {
            return GateVerdict::NothingToReview;
        };
        let Some(start) = self.stream_start else {
            return GateVerdict::Waiting;
        };
        self.held = None;
        if proposal.measured_frame < start {
            self.stale_rejected.fetch_add(1, Ordering::Relaxed);
            return GateVerdict::RejectedStale;
        }
        GateVerdict::Accepted(proposal)
    }

    /// Новый поток: метка и придержанное предложение прошлого потока забыты. Счётчик
    /// отброшенных живёт дольше потока (диагностика).
    pub(super) fn reset_for_new_stream(&mut self) {
        self.stream_start = None;
        self.held = None;
    }

    pub(super) fn stale_counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.stale_rejected)
    }
}
