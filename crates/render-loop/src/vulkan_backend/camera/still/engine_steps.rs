//! Шаги движка снимков на каждый показанный кадр (`StillEngine::advance`).
//!
//! Порядок шага: выбор кадра → опрос копии → запуск следующей копии → сроки цели → проявление → показ в кодер.
//! Опрос копии стоит ДО запуска следующей, чтобы закончившаяся копия освобождала слот в том же кадре.
//!
//! Запись переезжает между стадиями так: достаётся из очереди, обрабатывается по значению и возвращается на то же
//! место (`Some`) либо получает итог (`None`) — потерять запись, не доставив итог, шаг не может.

use std::sync::Arc;
use std::time::Duration;

use camera_contract::frame_passport::StreamPassport;
use isp_model::image::DisplayGamut;
use isp_vulkan::photo::RgbaReadback;
use still_contract::develop::{DevelopEvent, DevelopFailure, PrimaryTarget};
use still_contract::frame_choice::{FrameChoice, NoFrameReason};

use super::choice::{PickDetail, ShownAt, TranslatedPick, translate_pick};
use super::engine::StillEngine;
use super::entry::{DevelopInputs, EstimateBasis, PickPlan, Stage, StillEntry, StillOutcome};
use super::facts::{FactsRefusal, applied_lens_shading, build_facts};
use super::feed::ShownContent;
use super::gpu::{CapturePoll, DevelopProgress, DevelopStart, StillGpu};
use super::present_log::{PressResolution, Unprovable};
use super::step_timing::StillStep;
use crate::vulkan_backend::camera::isp_params::{IspParamsCache, ParamsOutcome};
use raw_frame_stream::RingPickRequest;
use std::time::Instant;

/// Что с GPU снимков перед копированием.
pub(super) enum GpuReadiness {
    Ready,
    /// Занят работой другого снимка, а кадру нужен GPU под другой поток: ждём.
    Busy,
    Unavailable(String),
}

impl<G: StillGpu> StillEngine<G> {
    /// Шаг на один показанный кадр видоискателя. Вызывается после показа кадра (полосы снимка встают в очередь GPU
    /// за кадром видоискателя, а не перед ним), а без показа — по таймеру потока рендера (решение 150). Шаги без
    /// экрана те же: выбор и изъятие кадра к этому моменту уже брошены отсоединением (`engine_surface`).
    pub fn advance(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        // Каждый шаг замеряется (`StillStepCpu`): всё, что он держит поток рендера, ждёт кадр видоискателя.
        self.timed(StillStep::Advance, |engine| {
            let now = engine.clock.now();
            engine.timed(StillStep::Choosing, |engine| engine.step_choosing(now));
            engine.timed(StillStep::Choosing, |engine| engine.step_flash_frames(now));
            engine.timed(StillStep::CapturePoll, |engine| engine.step_capture_poll(now));
            engine.timed(StillStep::CaptureStart, StillEngine::step_capture_start);
            engine.step_target_timeouts(now);
            engine.timed(StillStep::Develop, |engine| engine.step_develop(now));
            engine.timed(StillStep::Present, |engine| engine.step_present(now));
        });
        // Последний снимок без экрана получил итог: GPU снимков больше не нужен фоновому процессу.
        self.release_gpu_if_drained_without_surface();
    }

    /// Применяет `step` ко всем записям со стадией, подходящей `is_stage`, сохраняя порядок очереди.
    pub(super) fn for_each_in_stage(
        &mut self,
        is_stage: fn(&Stage<G>) -> bool,
        mut step: impl FnMut(&mut Self, StillEntry<G>) -> Option<StillEntry<G>>,
    ) {
        let mut index = 0;
        while index < self.queue.len() {
            if !is_stage(&self.queue[index].stage) {
                index += 1;
                continue;
            }
            let entry = self.queue.remove(index);
            if let Some(entry) = step(self, entry) {
                self.queue.insert(index, entry);
                index += 1;
            }
        }
    }

    /// Первая запись со стадией, подходящей `is_stage`, вынутая из очереди вместе с её местом.
    pub(super) fn take_first_in_stage(&mut self, is_stage: fn(&Stage<G>) -> bool) -> Option<(usize, StillEntry<G>)> {
        let index = self.queue.iter().position(|entry| is_stage(&entry.stage))?;
        Some((index, self.queue.remove(index)))
    }

    pub(super) fn put_back(&mut self, index: usize, entry: Option<StillEntry<G>>) {
        if let Some(entry) = entry {
            self.queue.insert(index.min(self.queue.len()), entry);
        }
    }

    fn any_in_stage(&self, is_stage: fn(&Stage<G>) -> bool) -> bool {
        self.queue.iter().any(|entry| is_stage(&entry.stage))
    }

    // ---- выбор кадра ----

    fn step_choosing(&mut self, now: Instant) {
        self.for_each_in_stage(
            |stage| matches!(stage, Stage::Choosing { .. }),
            |engine, entry| engine.choose(entry, now),
        );
    }

    fn choose(&mut self, mut entry: StillEntry<G>, now: Instant) -> Option<StillEntry<G>> {
        let Stage::Choosing { deadline, estimate } = entry.stage else {
            return Some(entry);
        };
        match self.present_log.resolve(entry.plan.press.at) {
            PressResolution::Proven {
                content: ShownContent::Live(sensor_timestamp),
                first_shown_at,
            } => {
                entry.stage = Stage::AwaitingCapture(PickPlan {
                    request: RingPickRequest::ShownFrame(sensor_timestamp),
                    shown_at: ShownAt::Proven(first_shown_at),
                });
                Some(entry)
            }
            PressResolution::Proven {
                content: ShownContent::NotLive,
                ..
            } => {
                self.counters.picks.no_frame_not_live_on_screen += 1;
                self.finish(entry, StillOutcome::NotSecured(NoFrameReason::NoLiveCameraFrame));
                None
            }
            PressResolution::Waiting if now < deadline => Some(entry),
            PressResolution::Waiting => {
                self.counters.presentation.wait_expired += 1;
                self.choose_by_estimate(entry, estimate)
            }
            PressResolution::Unprovable(reason) => {
                match reason {
                    Unprovable::GapInTimingRecords => self.counters.presentation.unprovable_gap += 1,
                    Unprovable::PressBeforeKnownTimings => {
                        self.counters.presentation.unprovable_press_before_timings += 1;
                    }
                    Unprovable::ShownContentForgotten => self.counters.presentation.unprovable_content_forgotten += 1,
                }
                self.choose_by_estimate(entry, estimate)
            }
        }
    }

    /// Момент нажатия доказать не удалось: кадр по оценке (последний живой на экране к приходу команды).
    fn choose_by_estimate(&mut self, mut entry: StillEntry<G>, estimate: EstimateBasis) -> Option<StillEntry<G>> {
        match estimate {
            EstimateBasis::Live(sensor_timestamp) => {
                entry.stage = Stage::AwaitingCapture(PickPlan {
                    request: RingPickRequest::NearestToEstimate(sensor_timestamp),
                    shown_at: ShownAt::Unknown,
                });
                Some(entry)
            }
            EstimateBasis::NoLiveFrame => {
                self.counters.picks.no_frame_nothing_shown += 1;
                self.finish(entry, StillOutcome::NotSecured(NoFrameReason::NoLiveCameraFrame));
                None
            }
        }
    }

    // ---- копия RAW ----

    /// Занят ли слот копии RAW: одна копия за раз (память), её ведёт GPU или запись в стадии `Capturing`.
    pub(super) fn capture_slot_busy(&self) -> bool {
        self.gpu.as_ref().is_some_and(StillGpu::capture_in_flight)
            || self.any_in_stage(|stage| matches!(stage, Stage::Capturing { .. }))
    }

    /// Готов ли GPU снимков к копии ДО того, как кадр изъят из кольца (изымать кадр, который некуда копировать,
    /// нельзя: он ушёл бы в вечный возврат). Поток — тот, на который смотрит видоискатель.
    pub(super) fn gpu_gate_before_pick(&mut self) -> GpuReadiness {
        if let Some(stream) = self.current_stream.clone() {
            self.ensure_gpu(&stream)
        } else if self.gpu.is_none() {
            GpuReadiness::Unavailable("потока кадров ещё нет".to_owned())
        } else {
            GpuReadiness::Ready
        }
    }

    fn step_capture_start(&mut self) {
        if self.capture_slot_busy() {
            return;
        }
        let Some((index, entry)) = self.take_first_in_stage(|stage| matches!(stage, Stage::AwaitingCapture(_))) else {
            return;
        };
        let entry = self.start_capture(entry);
        self.put_back(index, entry);
    }

    fn start_capture(&mut self, entry: StillEntry<G>) -> Option<StillEntry<G>> {
        let Stage::AwaitingCapture(plan) = entry.stage else {
            return Some(entry);
        };
        // GPU под поток, на который смотрит видоискатель, готовят заранее; здесь последняя проверка до изъятия кадра.
        match self.gpu_gate_before_pick() {
            GpuReadiness::Ready => {}
            GpuReadiness::Busy => return Some(entry),
            GpuReadiness::Unavailable(reason) => return self.refuse_capture_gpu(entry, &reason),
        }
        let pick = translate_pick(self.ring.pick(plan.request), plan.shown_at);
        let (lease, choice, detail) = match pick {
            TranslatedPick::Frame { lease, choice, detail } => (lease, choice, detail),
            TranslatedPick::NoFrame { reason, detail } => {
                self.count_no_frame(reason, detail);
                self.finish(entry, StillOutcome::NotSecured(reason));
                return None;
            }
        };
        self.count_choice(&choice, detail);
        if let FrameChoice::ShownAtPress { shown_at, .. } = &choice {
            // Обе метки — `MONOTONIC` (нажатие и первый показ кадра): вычитание корректно. Метку сенсора (`BOOTTIME`)
            // с ними не смешиваем никогда.
            let after_shown_ms = (entry.plan.press.at.as_nanos() - shown_at.as_nanos()) as f64 / 1_000_000.0;
            self.counters.exact_press_after_shown.observe(after_shown_ms);
        }
        self.launch_capture(entry, lease, choice, Stage::AwaitingCapture(plan))
    }

    /// Общая часть запуска копии для выбранного кадра (обычный снимок и кадр со вспышкой): GPU под поток кадра,
    /// факты и параметры, старт копии. `retry` — стадия, в которую запись возвращается, если GPU занят другим
    /// снимком (кадр отпускается нетронутым и берётся заново).
    pub(super) fn launch_capture(
        &mut self,
        mut entry: StillEntry<G>,
        lease: raw_frame_stream::RawFrameLease,
        choice: FrameChoice,
        retry: Stage<G>,
    ) -> Option<StillEntry<G>> {
        // Кадр мог оказаться из другого потока, чем тот, под который готов GPU (смена объектива между шагами).
        let stream = Arc::clone(&lease.passport().stream);
        match self.ensure_gpu(&stream) {
            GpuReadiness::Ready => {}
            GpuReadiness::Busy => {
                // GPU занят другим снимком и не годится этому кадру: кадр возвращаем нетронутым, ждём.
                lease.release_unused();
                entry.stage = retry;
                return Some(entry);
            }
            GpuReadiness::Unavailable(reason) => {
                lease.release_unused();
                return self.refuse_capture_gpu(entry, &reason);
            }
        }
        let prepared = self.prepare_frame(&entry, &lease, choice);
        let (facts, params) = match prepared {
            Ok(prepared) => prepared,
            Err(refusal) => {
                lease.release_unused();
                self.finish(entry, StillOutcome::NotSecured(refusal));
                return None;
            }
        };
        let Some(gpu) = self.gpu.as_mut() else {
            lease.release_unused();
            return self.refuse_capture_gpu(entry, "GPU снимков потерян");
        };
        match gpu.capture_start(lease) {
            Ok(()) => {
                self.counters.captures_started += 1;
                let deadline = self.clock.now() + Duration::from_millis(u64::from(self.config.capture_timeout_ms));
                entry.stage = Stage::Capturing {
                    facts,
                    params,
                    deadline,
                };
                Some(entry)
            }
            Err(error) => {
                tracing::warn!(target: super::LOG_TARGET, %error, "копия RAW снимка не запустилась");
                self.counters.refusals.capture_start_failed += 1;
                self.finish(entry, StillOutcome::NotSecured(NoFrameReason::CaptureFailed));
                None
            }
        }
    }

    fn refuse_capture_gpu(&mut self, entry: StillEntry<G>, reason: &str) -> Option<StillEntry<G>> {
        tracing::warn!(target: super::LOG_TARGET, reason, "GPU снимков недоступен");
        self.counters.refusals.gpu_unavailable += 1;
        self.finish(entry, StillOutcome::NotSecured(NoFrameReason::CaptureFailed));
        None
    }

    /// Факты кадра и параметры ISP снимка. Отказ — «кадра нет» (до захвата, затвор не освобождался).
    fn prepare_frame(
        &mut self,
        entry: &StillEntry<G>,
        lease: &raw_frame_stream::RawFrameLease,
        choice: FrameChoice,
    ) -> Result<(Box<still_contract::shot::StillFrameFacts>, Box<IspParamsCache>), NoFrameReason> {
        let passport = lease.passport();
        let params = match choice {
            // Кадр со вспышкой проявляется параметрами плана движка камеры и белой светодиода, а не записью показа.
            FrameChoice::FlashFrame { shot, .. } => self.flash_frame_params(shot, passport)?,
            FrameChoice::ShownAtPress { .. } | FrameChoice::Substituted { .. } => {
                self.viewfinder_params_for(passport)?
            }
        };
        // Параметры снимка: Quality-демозаик, охват sRGB (решение 142) независимо от охвата экрана, тон — тот же,
        // что у видоискателя (`viewfinder.tone`). Годятся только свежепосчитанные: «прежние после отказа» — чужие.
        // Считаются ДО фактов: решение стадии виньетирования из этих же параметров уходит в факты (DNG
        // описывает ту поправку, что применена к картинке, решение 8).
        let mut cache = IspParamsCache::for_demosaic(isp_model::demosaic::DemosaicMethod::Quality)
            .with_tone(self.tone)
            // Основа — модель НА МОМЕНТ НАЖАТИЯ из плана (решение 195), а не `viewfinder.tone.base` и не основа
            // видоискателя сейчас: после нажатия свайп основы снимок не меняет. Секции моделей берутся из конфига.
            .with_tone_base_model(entry.plan.tone_base)
            .with_lens_shading(self.lens_shading)
            .with_look_model(Arc::clone(&self.look_model))
            // Вид — значения НА МОМЕНТ НАЖАТИЯ из плана снимка (инвариант 3), а не текущий вид видоискателя; для кадра
            // со вспышкой тоже: параметры вспышки (белая, подтяжка) выше этого не меняют.
            .with_look(Arc::new(entry.plan.look.clone()));
        let decision = cache.prepare_with_exposure_gain(passport, params.white, params.gain, DisplayGamut::Srgb);
        let resolved = match decision.outcome {
            ParamsOutcome::Resolved | ParamsOutcome::Unchanged => cache.current(),
            ParamsOutcome::ReusedAfterFailure(_) | ParamsOutcome::Unavailable(_) => None,
        };
        let Some(isp_params) = resolved else {
            self.counters.params.unresolved += 1;
            return Err(NoFrameReason::FrameUnusable);
        };
        let facts = applied_lens_shading(&isp_params.lens_shading)
            .and_then(|lens_shading| build_facts(&entry.plan, passport, choice, &params, lens_shading));
        let facts = match facts {
            Ok(facts) => facts,
            Err(refusal) => {
                match refusal {
                    FactsRefusal::NoCaptureDetails => self.counters.refusals.facts_no_capture_details += 1,
                    FactsRefusal::CameraNotInCatalog => self.counters.refusals.facts_camera_not_in_catalog += 1,
                    FactsRefusal::Crop(_) => self.counters.refusals.facts_crop += 1,
                    FactsRefusal::Color(_) => self.counters.refusals.facts_color += 1,
                }
                tracing::warn!(target: super::LOG_TARGET, %refusal, "факты кадра снимка не собраны");
                return Err(NoFrameReason::FrameUnusable);
            }
        };
        Ok((Box::new(facts), Box::new(cache)))
    }

    fn count_no_frame(&mut self, reason: NoFrameReason, detail: PickDetail) {
        let picks = &mut self.counters.picks;
        match (reason, detail) {
            (NoFrameReason::RingDisabled, _) => picks.no_frame_ring_disabled += 1,
            (NoFrameReason::RingEmpty, PickDetail::NoEligibleFrame) => picks.no_frame_no_eligible += 1,
            (NoFrameReason::RingEmpty, _) => picks.no_frame_ring_empty += 1,
            (NoFrameReason::NoLiveCameraFrame, _) => picks.no_frame_nothing_shown += 1,
            // Эти причины считаются своими счётчиками в месте отказа (`refusals`, `params`), здесь — не выбор кадра.
            (
                NoFrameReason::RendererBusy
                | NoFrameReason::FrameUnusable
                | NoFrameReason::CaptureFailed
                | NoFrameReason::RendererStopped
                | NoFrameReason::FlashNotCaptured(_)
                | NoFrameReason::FlashFrameNeverArrived
                | NoFrameReason::FlashFrameNotLit,
                _,
            ) => {}
        }
    }

    fn count_choice(&mut self, choice: &FrameChoice, detail: PickDetail) {
        use still_contract::frame_choice::SubstitutionReason as Reason;
        let picks = &mut self.counters.picks;
        match choice {
            FrameChoice::ShownAtPress { .. } => picks.exact += 1,
            // Кадр со вспышкой считается своим счётчиком (`flash.frames_found`), не выбором «что было на экране».
            FrameChoice::FlashFrame { .. } => {}
            FrameChoice::Substituted { reason, .. } => match (reason, detail) {
                (Reason::PresentationTimingUnknown, _) => picks.substituted_timing_unknown += 1,
                (Reason::ShownFrameEvicted, PickDetail::TargetLostBeforeRing) => {
                    picks.substituted_lost_before_ring += 1;
                }
                (Reason::ShownFrameEvicted, _) => picks.substituted_evicted += 1,
                (Reason::ShownFrameWithoutMetadata, _) => picks.substituted_without_metadata += 1,
                (Reason::ShownFrameNotRegular, _) => picks.substituted_not_regular += 1,
            },
        }
    }

    fn step_capture_poll(&mut self, now: Instant) {
        let Some((index, entry)) = self.take_first_in_stage(|stage| matches!(stage, Stage::Capturing { .. })) else {
            return;
        };
        let polled = match self.gpu.as_mut() {
            Some(gpu) => gpu.capture_poll(),
            None => CapturePoll::Failed("GPU снимков потерян".to_owned()),
        };
        let deadline = match &entry.stage {
            Stage::Capturing { deadline, .. } => *deadline,
            _ => now,
        };
        let entry = match polled {
            CapturePoll::Pending if now < deadline => Some(entry),
            CapturePoll::Pending => {
                // Fence копии не сигналит в срок: GPU мог ещё читать кадр, поэтому лизинг камере не возвращается
                // (учтённая утечка), а GPU снимков объявляется потерянным.
                tracing::error!(target: super::LOG_TARGET, "копия RAW снимка не завершилась в срок");
                self.counters.gpu_lost_by_deadline += 1;
                self.lose_gpu(entry, "копия RAW не завершилась за отведённое время".to_owned());
                None
            }
            CapturePoll::NothingInFlight => {
                tracing::error!(target: super::LOG_TARGET, "запись ждёт копию, а GPU копии не ведёт");
                self.counters.refusals.capture_failed += 1;
                self.finish(entry, StillOutcome::NotSecured(NoFrameReason::CaptureFailed));
                None
            }
            CapturePoll::Failed(error) => {
                tracing::warn!(target: super::LOG_TARGET, %error, "копия RAW снимка сорвалась");
                self.counters.refusals.capture_failed += 1;
                self.finish(entry, StillOutcome::NotSecured(NoFrameReason::CaptureFailed));
                None
            }
            CapturePoll::Lost(error) => {
                tracing::error!(target: super::LOG_TARGET, %error, "опрос копии RAW не удался: исход на GPU неизвестен");
                self.counters.refusals.capture_failed += 1;
                self.lose_gpu(entry, error);
                None
            }
            CapturePoll::Ready(raw) => self.secure(entry, raw),
        };
        self.put_back(index, entry);
    }

    /// Копия готова: кадр уже у камеры, затвор свободен — сообщаем `Secured` и ждём цель от записи.
    fn secure(&mut self, mut entry: StillEntry<G>, raw: G::Raw) -> Option<StillEntry<G>> {
        self.counters.captures_completed += 1;
        let Stage::Capturing { facts, params, .. } = entry.take_stage() else {
            return Some(entry);
        };
        // Кадр со вспышкой: HAL мог сообщить `FIRED` на тёмном кадре — до `Secured` проверяем по пикселям.
        if let Some(reason) = self.reject_unlit_flash_frame(&raw, &facts, &params) {
            self.finish(entry, StillOutcome::NotSecured(reason));
            return None;
        }
        let samples = if facts.outputs.needs_raw_samples() {
            let extracted = match self.gpu.as_ref() {
                Some(gpu) => gpu.raw_samples(&raw),
                None => Err("GPU снимков потерян".to_owned()),
            };
            match extracted {
                Ok(samples) => Some(samples),
                Err(error) => {
                    // Кадр скопирован, но отдать сырые отсчёты нельзя, а `Secured` ещё не отправлялся: по протоколу
                    // `still_contract::develop` `Failed` допустим только ПОСЛЕ `Secured`, поэтому итог — «кадра нет».
                    tracing::warn!(target: super::LOG_TARGET, %error, "сырые отсчёты копии не прочитаны");
                    self.counters.refusals.raw_samples_unreadable += 1;
                    self.finish(entry, StillOutcome::NotSecured(NoFrameReason::CaptureFailed));
                    return None;
                }
            }
        } else {
            None
        };
        entry.deliver_secured(DevelopEvent::Secured {
            facts: facts.clone(),
            raw: samples,
        });
        let deadline = self.clock.now() + Duration::from_millis(u64::from(self.config.target_timeout_ms));
        entry.stage = Stage::AwaitingTarget {
            deadline,
            inputs: DevelopInputs { raw, facts, params },
        };
        Some(entry)
    }

    // ---- ожидание цели ----

    fn step_target_timeouts(&mut self, now: Instant) {
        self.for_each_in_stage(
            |stage| matches!(stage, Stage::AwaitingTarget { .. }),
            |engine, entry| match entry.stage {
                Stage::AwaitingTarget { deadline, .. } if now >= deadline => {
                    engine.finish(entry, StillOutcome::Failed(DevelopFailure::TargetTimedOut));
                    None
                }
                _ => Some(entry),
            },
        );
    }

    // ---- проявление ----

    fn step_develop(&mut self, now: Instant) {
        if !self.any_in_stage(|stage| matches!(stage, Stage::Developing { .. }))
            && let Some((index, entry)) =
                self.take_first_in_stage(|stage| matches!(stage, Stage::AwaitingDevelop { .. }))
        {
            let entry = self.start_develop(entry, now);
            self.put_back(index, entry);
        }
        if let Some((index, entry)) = self.take_first_in_stage(|stage| matches!(stage, Stage::Developing { .. })) {
            let entry = self.advance_develop(entry, now);
            self.put_back(index, entry);
        }
    }

    fn start_develop(&mut self, mut entry: StillEntry<G>, now: Instant) -> Option<StillEntry<G>> {
        let Stage::AwaitingDevelop { target, inputs } = entry.take_stage() else {
            return Some(entry);
        };
        let readback = match &target {
            PrimaryTarget::Readback => RgbaReadback::ForJpeg,
            PrimaryTarget::EncoderSurface(_) => RgbaReadback::NotNeeded,
        };
        let Some(isp_params) = inputs.params.current() else {
            self.finish(
                entry,
                StillOutcome::Failed(DevelopFailure::ParamsUnavailable(
                    "параметры ISP снимка потеряны".to_owned(),
                )),
            );
            return None;
        };
        let Some(gpu) = self.gpu.as_mut() else {
            self.finish(
                entry,
                StillOutcome::Failed(DevelopFailure::Gpu("GPU снимков потерян".to_owned())),
            );
            return None;
        };
        let begin_started = self.clock.now();
        let started = gpu.develop_start(DevelopStart {
            raw: &inputs.raw,
            params: isp_params,
            crop: inputs.facts.crop,
            readback,
            rotation: inputs.facts.rotation,
        });
        self.record_step_cpu(StillStep::DevelopBegin, begin_started);
        match started {
            Ok(()) => {
                self.counters.developments_started += 1;
                entry.stage = Stage::Developing {
                    target,
                    started: now,
                    _inputs: inputs,
                };
                Some(entry)
            }
            Err(error) => {
                self.finish(entry, StillOutcome::Failed(DevelopFailure::Gpu(error)));
                None
            }
        }
    }

    fn advance_develop(&mut self, mut entry: StillEntry<G>, now: Instant) -> Option<StillEntry<G>> {
        let slices = self.config.slices_per_frame;
        let step_started = self.clock.now();
        let progress = match self.gpu.as_mut() {
            Some(gpu) => gpu.develop_advance(slices),
            None => DevelopProgress::Failed("GPU снимков потерян".to_owned()),
        };
        let step_cpu = self.clock.now().saturating_duration_since(step_started);
        self.counters.develop_step_cpu.observe(step_cpu.as_secs_f64() * 1_000.0);
        match progress {
            DevelopProgress::InFlight { submitted_now } => {
                self.counters.slices_submitted += u64::from(submitted_now);
                let started = match &entry.stage {
                    Stage::Developing { started, .. } => *started,
                    _ => now,
                };
                let timeout = Duration::from_millis(u64::from(self.config.develop_timeout_ms));
                if now.saturating_duration_since(started) >= timeout {
                    // Проявление не кончается за срок: fence полос завис, GPU снимков потерян, память не освобождаем.
                    tracing::error!(target: super::LOG_TARGET, "проявление снимка не завершилось в срок");
                    self.counters.gpu_lost_by_deadline += 1;
                    self.lose_gpu(entry, "проявление не завершилось за отведённое время".to_owned());
                    return None;
                }
                Some(entry)
            }
            DevelopProgress::Lost(error) => {
                tracing::error!(target: super::LOG_TARGET, %error, "опрос полосы не удался: исход на GPU неизвестен");
                self.lose_gpu(entry, error);
                None
            }
            DevelopProgress::Idle => {
                self.finish(
                    entry,
                    StillOutcome::Failed(DevelopFailure::Gpu("проявление пропало у GPU".to_owned())),
                );
                None
            }
            DevelopProgress::Failed(error) => {
                self.finish(entry, StillOutcome::Failed(DevelopFailure::Gpu(error)));
                None
            }
            DevelopProgress::Done(image) => {
                self.counters.developments_completed += 1;
                let Stage::Developing { target, started, .. } = entry.take_stage() else {
                    return Some(entry);
                };
                self.counters
                    .develop_wall
                    .observe(now.saturating_duration_since(started).as_secs_f64() * 1_000.0);
                // Миниатюра строго раньше итога (14g): после `deliver_developed` итог уже может уйти.
                let mut image = image;
                self.deliver_thumbnail_before_final(&mut entry, &mut image);
                self.deliver_developed(entry, target, image)
            }
        }
    }

    /// Картинка готова: в память (JPEG) или в поверхность кодера (HEIF).
    fn deliver_developed(
        &mut self,
        mut entry: StillEntry<G>,
        target: PrimaryTarget,
        mut image: G::Image,
    ) -> Option<StillEntry<G>> {
        match target {
            PrimaryTarget::Readback => {
                let readback = self.gpu.as_ref().and_then(|gpu| gpu.take_readback(&mut image));
                match readback {
                    Some(picture) => self.finish(entry, StillOutcome::Readback(picture)),
                    None => self.finish(
                        entry,
                        StillOutcome::Failed(DevelopFailure::Gpu("картинки для JPEG в памяти нет".to_owned())),
                    ),
                }
                None
            }
            PrimaryTarget::EncoderSurface(surface) => {
                let present_busy = self.gpu.as_ref().is_some_and(StillGpu::present_in_flight);
                if present_busy {
                    entry.stage = Stage::Developed { surface, image };
                    Some(entry)
                } else {
                    self.start_present(entry, surface, image)
                }
            }
        }
    }

    // ---- GPU ----

    /// GPU под поток: уже годный, пересоздаваемый (если простаивает) или занятый.
    pub(super) fn ensure_gpu(&mut self, stream: &Arc<StreamPassport>) -> GpuReadiness {
        if let Some(gpu) = &self.gpu {
            if gpu.serves(stream) {
                return GpuReadiness::Ready;
            }
            if gpu.capture_in_flight() || gpu.develop_in_flight() || gpu.present_in_flight() {
                return GpuReadiness::Busy;
            }
        }
        // Замер только на пути пересоздания (быстрый путь «GPU уже годный» выше не меряется): он включает разбор
        // старого GPU и создание нового, всё это идёт на потоке рендера.
        let creation_started = self.clock.now();
        // GPU простаивает (или его нет): старый разбирается (`retire_gpu`), новый создаётся под этот поток.
        if let Some(old) = self.gpu.take() {
            self.retire_gpu(old);
        }
        let made = (self.maker)(stream);
        self.record_step_cpu(StillStep::GpuCreation, creation_started);
        match made {
            Ok(gpu) => {
                self.counters.gpu_created += 1;
                self.gpu = Some(gpu);
                GpuReadiness::Ready
            }
            Err(error) => GpuReadiness::Unavailable(error),
        }
    }

    /// Готовит GPU под поток, который показывает видоискатель, ПОКА снимков нет: создание пайплайнов и буферов
    /// (десятки мс) не должно случиться в момент затвора.
    pub(super) fn prewarm_gpu(&mut self) {
        let Some(stream) = self.current_stream.clone() else {
            return;
        };
        if !self.queue.is_empty()
            || self
                .prewarm_failed_for
                .as_ref()
                .is_some_and(|failed| Arc::ptr_eq(failed, &stream))
        {
            return;
        }
        match self.ensure_gpu(&stream) {
            GpuReadiness::Ready | GpuReadiness::Busy => {}
            GpuReadiness::Unavailable(error) => {
                tracing::warn!(target: super::LOG_TARGET, %error, "GPU снимков заранее не создан: повтор при нажатии");
                self.counters.gpu_prewarm_failures += 1;
                self.prewarm_failed_for = Some(stream);
            }
        }
    }
}
