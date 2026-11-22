//! Рабочий поток службы: единственный, кто трогает хранилище, кодеры и файловые форматы.
//!
//! Один снимок идёт по состояниям `AwaitingCapture → Developing → (Saved | Failed)`; события рендера
//! для снимка, которого уже нет (срок вышел, служба остановлена), считаются «опоздавшими» и
//! игнорируются. Итог снимка фиксирует [`Worker::conclude`]: сначала учёт и освобождение слота
//! допуска, потом фаза — чтобы приложение, увидев итог, сразу могло допустить следующий снимок.

mod finish;
mod job;
mod secure;

use std::cell::Cell;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Instant;

use still_contract::develop::{DevelopEvent, PrimaryTargetOffer};
use still_contract::phase::{SavedStill, StillFailure, StillPhase};
use still_contract::plan::StillId;

use self::job::{Job, Stage};
use super::StillServiceDeps;
use super::admission::{AdmissionTicket, StillAdmission};
use super::config::StillServiceConfig;
use super::inbox::{Command, Inbox};
use super::stats::StillCounters;
use crate::exif::ExifInputs;

pub(super) struct Worker {
    config: StillServiceConfig,
    deps: StillServiceDeps,
    counters: Arc<StillCounters>,
    admission: Arc<StillAdmission>,
    inbox: Arc<Inbox>,
    jobs: HashMap<StillId, Job>,
    /// Снимок, который поток обрабатывает прямо сейчас (снят из таблицы, итога ещё нет). Нужен
    /// только ради паники: раскрутка стека уничтожает его билет, и без этой записи снимок остался
    /// бы без итоговой фазы. [`Worker::conclude`] и [`Worker::put_job`] её сбрасывают.
    in_flight: Cell<Option<StillId>>,
}

impl Worker {
    pub(super) fn new(
        config: StillServiceConfig,
        deps: StillServiceDeps,
        counters: Arc<StillCounters>,
        admission: Arc<StillAdmission>,
        inbox: Arc<Inbox>,
    ) -> Self {
        Self {
            config,
            deps,
            counters,
            admission,
            inbox,
            jobs: HashMap::new(),
            in_flight: Cell::new(None),
        }
    }

    /// Поток службы целиком: цикл команд под защитой от паники, потом окончательная остановка.
    ///
    /// Паника в платформенном вызове или коде формата не должна оставлять принятые снимки без
    /// итога: после неё служба останавливается (дальше работать с неясным состоянием нельзя), а
    /// все живые снимки получают `Failed(ServiceUnavailable)`.
    pub(super) fn run(mut self, commands: &Receiver<Command>) {
        if catch_unwind(AssertUnwindSafe(|| self.serve(commands))).is_err() {
            self.counters.record_worker_panic();
            tracing::error!("рабочий поток службы снимков поймал панику: служба останавливается");
            self.fail_in_flight_after_panic();
        }
        self.stop_for_good(commands);
    }

    /// Цикл команд: ждёт команду не дольше ближайшего срока, потом снимает просроченные снимки.
    fn serve(&mut self, commands: &Receiver<Command>) {
        loop {
            let received = match self.nearest_deadline() {
                Some(deadline) => commands.recv_timeout(deadline.saturating_duration_since(Instant::now())),
                None => commands.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match received {
                Ok(Command::BeginStill(ticket)) => self.begin_still(ticket),
                Ok(Command::Develop(event)) => {
                    self.inbox.mark_event_taken();
                    self.on_develop_event(event);
                }
                Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.expire_overdue(Instant::now());
        }
    }

    fn nearest_deadline(&self) -> Option<Instant> {
        self.jobs.values().map(|job| job.stage.deadline()).min()
    }

    fn begin_still(&mut self, ticket: AdmissionTicket) {
        let id = ticket.still_id();
        let deadline = Instant::now() + self.config.capture_timeout();
        self.jobs.insert(
            id,
            Job {
                ticket,
                stage: Stage::AwaitingCapture { deadline },
            },
        );
    }

    fn on_develop_event(&mut self, event: DevelopEvent) {
        match event {
            DevelopEvent::Secured { facts, raw } => self.on_secured(facts, raw),
            DevelopEvent::NotSecured { id, reason } => self.on_not_secured(id, reason),
            DevelopEvent::PrimaryRendered { id } => self.on_primary_rendered(id),
            DevelopEvent::PrimaryReadback { id, image } => self.on_primary_readback(id, &image),
            DevelopEvent::Failed { id, failure } => self.on_render_failed(id, failure.into()),
        }
    }

    /// Снимает снимок с таблицы и помечает его «в обработке» (см. `in_flight`).
    fn take_job(&mut self, id: StillId) -> Option<Job> {
        let job = self.jobs.remove(&id);
        if job.is_some() {
            self.in_flight.set(Some(id));
        }
        job
    }

    /// Возвращает снимок в таблицу: он снова ждёт события рендера, «в обработке» больше нет.
    fn put_job(&mut self, id: StillId, job: Job) {
        self.jobs.insert(id, job);
        self.in_flight.set(None);
    }

    /// Забирает снимок из таблицы; если его нет — событие опоздало: считаем и игнорируем.
    fn take_job_or_count_late(&mut self, id: StillId) -> Option<Job> {
        let job = self.take_job(id);
        if job.is_none() {
            self.counters.record_late_event();
        }
        job
    }

    fn on_not_secured(&mut self, id: StillId, reason: still_contract::frame_choice::NoFrameReason) {
        if let Some(job) = self.take_job_or_count_late(id) {
            self.fail_job(id, job, StillFailure::NoFrame(reason));
        }
    }

    fn on_render_failed(&mut self, id: StillId, failure: StillFailure) {
        if let Some(job) = self.take_job_or_count_late(id) {
            self.fail_job(id, job, failure);
        }
    }

    /// Срывает снимок: освобождает ресурсы, фиксирует итог.
    fn fail_job(&self, id: StillId, job: Job, failure: StillFailure) {
        let Job { ticket, stage } = job;
        self.fail_parts(id, ticket, stage, failure);
    }

    fn fail_parts(&self, id: StillId, ticket: AdmissionTicket, stage: Stage, failure: StillFailure) {
        if let Stage::Developing(developing) = stage {
            developing.release_everything(&self.counters);
        }
        self.conclude(id, ticket, Err(failure));
    }

    /// Единственное место, где снимок получает итог: учёт → слот допуска → фаза.
    fn conclude(&self, id: StillId, ticket: AdmissionTicket, outcome: Result<SavedStill, StillFailure>) {
        let phase = match outcome {
            Ok(files) => {
                self.counters.record_saved();
                StillPhase::Saved { id, files }
            }
            Err(failure) => {
                self.counters.record_failure(&failure);
                StillPhase::Failed { id, failure }
            }
        };
        drop(ticket);
        self.in_flight.set(None);
        self.deps.phases.report_phase(phase);
    }

    /// Снимает снимки, у которых вышел срок.
    fn expire_overdue(&mut self, now: Instant) {
        let overdue: Vec<StillId> = self
            .jobs
            .iter()
            .filter(|(_, job)| job.stage.deadline() <= now)
            .map(|(id, _)| *id)
            .collect();
        for id in overdue {
            let Some(job) = self.take_job(id) else { continue };
            let reason = match job.stage {
                Stage::AwaitingCapture { .. } => format!("кадр не захвачен за {} мс", self.config.capture_timeout_ms),
                Stage::Developing(_) => format!("проявление не закончено за {} мс", self.config.develop_timeout_ms),
            };
            self.counters.record_timeout();
            tracing::warn!(?id, %reason, "снимок сорван по сроку");
            self.fail_job(id, job, StillFailure::Develop(reason));
        }
    }

    /// Окончательная остановка: незаконченные снимки срываются, допуск и очередь закрываются, а
    /// всё, что успело лечь в очередь за время остановки, получает честный итог.
    fn stop_for_good(&mut self, commands: &Receiver<Command>) {
        self.abandon_everything();
        for command in self.inbox.close_for_good(commands) {
            match command {
                Command::BeginStill(ticket) => {
                    // Приложение успело отдать билет, но снимок так и не начался.
                    let id = ticket.still_id();
                    self.conclude(id, ticket, Err(StillFailure::ServiceUnavailable));
                }
                Command::Develop(event) => {
                    self.inbox.mark_event_taken();
                    self.counters.record_late_event();
                    if let DevelopEvent::Secured { facts, .. } = &event {
                        // Рендер ждёт цель: без ответа он не отпустит захваченный кадр.
                        self.offer_unavailable(facts.id);
                    }
                    // Событие уничтожается здесь: его поверхность и картинка отпускаются `Drop`.
                    drop(event);
                }
                Command::Shutdown => {}
            }
        }
    }

    /// Все снимки из таблицы срываются (допуск закрывается первым: новые билеты не выдаются).
    fn abandon_everything(&mut self) {
        self.admission.close();
        let ids: Vec<StillId> = self.jobs.keys().copied().collect();
        for id in ids {
            if let Some(job) = self.take_job(id) {
                self.fail_job(id, job, StillFailure::ServiceUnavailable);
            }
        }
    }

    /// После паники: снимок, который был «в обработке», потерял билет при раскрутке стека, но итога
    /// не получил, — даём ему итог. Если снимок ещё в таблице, его сорвёт `abandon_everything`.
    fn fail_in_flight_after_panic(&mut self) {
        let Some(id) = self.in_flight.take() else { return };
        if self.jobs.contains_key(&id) {
            return;
        }
        let failure = StillFailure::ServiceUnavailable;
        self.counters.record_failure(&failure);
        self.deps.phases.report_phase(StillPhase::Failed { id, failure });
    }

    /// Собирает блок Exif. Метаданные необязательны: сбой не срывает снимок, файл пишется без Exif,
    /// а потеря не молчит — лог `error` и счётчик `exif_build_failures`.
    fn build_exif_or_skip(&self, inputs: &ExifInputs<'_>) -> Option<Vec<u8>> {
        match self.deps.formats.build_exif(inputs) {
            Ok(block) => Some(block),
            Err(error) => {
                self.counters.record_exif_build_failure();
                tracing::error!(%error, id = ?inputs.facts.id, "Exif не собран: снимок записывается без метаданных");
                None
            }
        }
    }

    /// Отвечает рендеру «цели нет»: он отпустит снимок.
    fn offer_unavailable(&self, id: StillId) {
        let offer = PrimaryTargetOffer::Unavailable { id };
        if self.deps.targets.offer_primary_target(offer).is_err() {
            tracing::warn!(?id, "рендер не принял отказ в цели: он остановлен");
        }
    }
}
