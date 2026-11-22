//! Снимок со вспышкой в движке снимков (сессия 14d, решения 158, 159).
//!
//! # Как идёт снимок со вспышкой
//! `Begin` с `FrameSelection::FlashFrame { shot }` не ждёт показов видоискателя: кадр снимка — не «что было
//! на экране», а кадр разового запроса камеры. Запись встаёт в стадию [`Stage::AwaitingFlashFrame`] со сроком
//! `still.flash_frame_wait_ms`. Каждый шаг: найти кадр в кольце ПО МЕТКЕ запроса (`pick_flash_frame`, замены
//! другим кадром НЕТ) → копия RAW общим путём (`launch_capture`) → проверка света по пикселям ДО `Secured`.
//!
//! # Исходы (ровно один на `Begin`, как у обычного снимка)
//! * кадр не дошёл до срока — `FlashFrameNeverArrived`; движок камеры сообщил отказ — `FlashNotCaptured(причина)`;
//! * кадр дошёл, но не освещён — `FlashFrameNotLit`; нет плана или выдержки кадра — `FrameUnusable`;
//! * отсоединение surface — `RendererStopped` (стадия нуждается в кольце и видоискателе, `engine_surface`).
//!
//! # Параметры кадра
//! Мимо истории видоискателя: подтяжка = план движка / произведение кадра (округление и предел те же, что у
//! `k` видоискателя), белая = оценка кадра с фонариком (тот же светодиод); нет её — белая видоискателя с
//! явным счётчиком.

use std::sync::Arc;
use std::time::{Duration, Instant};

use camera_contract::auto_exposure::ExposureProduct;
use camera_contract::flash::{FlashShotId, FlashStillFailure};
use camera_contract::frame_passport::RawFramePassport;
use still_contract::frame_choice::{FrameChoice, NoFrameReason};
use still_contract::plan::{FrameSelection, StillId};
use still_contract::shot::StillFrameFacts;

use super::engine::StillEngine;
use super::engine_steps::GpuReadiness;
use super::entry::{Stage, StillEntry, StillOutcome};
use super::feed::FrameStillParams;
use super::flash_lit::{GreenSampling, LitThresholds, LitUndecidable, LitVerdict, judge_lit, mean_green_level};
use super::gpu::StillGpu;
use crate::vulkan_backend::camera::exposure::frame_exposure_product;
use crate::vulkan_backend::camera::isp_params::IspParamsCache;
use raw_frame_stream::FlashRingPick;

impl<G: StillGpu> StillEngine<G> {
    /// Стадия новой записи по плану: обычный снимок ждёт доказательства показа, снимок со вспышкой — кадр запроса.
    pub(super) fn flash_wait_deadline(&self) -> Instant {
        self.clock.now() + Duration::from_millis(u64::from(self.config.flash_frame_wait_ms))
    }

    /// Номер снимка со вспышкой плана (`None` — обычный снимок).
    pub(super) fn flash_shot_of(selection: FrameSelection) -> Option<FlashShotId> {
        match selection {
            FrameSelection::FlashFrame { shot } => Some(shot),
            FrameSelection::ShownAtPress => None,
        }
    }

    // ---- ожидание кадра ----

    /// Шаг записей, ждущих кадр со вспышкой.
    pub(super) fn step_flash_frames(&mut self, now: Instant) {
        self.for_each_in_stage(
            |stage| matches!(stage, Stage::AwaitingFlashFrame { .. }),
            |engine, entry| engine.flash_frame_step(entry, now),
        );
    }

    fn flash_frame_step(&mut self, mut entry: StillEntry<G>, now: Instant) -> Option<StillEntry<G>> {
        let Stage::AwaitingFlashFrame { shot, deadline } = entry.stage else {
            return Some(entry);
        };
        // Слот копии занят — кадр не изымаем (каждый повтор считал бы выдачу доли): ждём освобождения слота,
        // но не дольше срока записи (занятость не продлевает ожидание кадра).
        if self.capture_slot_busy() {
            return self.keep_waiting_for_free_gpu(entry, now, deadline);
        }
        match self.gpu_gate_before_pick() {
            GpuReadiness::Ready => {}
            GpuReadiness::Busy => return self.keep_waiting_for_free_gpu(entry, now, deadline),
            GpuReadiness::Unavailable(reason) => {
                tracing::warn!(target: super::LOG_TARGET, reason, "GPU снимков недоступен");
                self.counters.refusals.gpu_unavailable += 1;
                self.finish(entry, StillOutcome::NotSecured(NoFrameReason::CaptureFailed));
                return None;
            }
        }
        match self.ring.pick_flash_frame(shot) {
            FlashRingPick::Found(lease) => {
                // Запись может найти кадр не раз (GPU отдал кадр обратно, повтор): считаем кадр один раз.
                if entry.note_flash_frame_found() {
                    self.counters.flash.frames_found += 1;
                }
                let choice = FrameChoice::FlashFrame {
                    sensor_timestamp: lease.passport().sensor_timestamp,
                    shot,
                };
                let waiting =
                    self.launch_capture(entry, lease, choice, Stage::AwaitingFlashFrame { shot, deadline })?;
                // GPU занят другим снимком и кадр вернулся в кольцо: ждём, но не дольше срока записи.
                if matches!(waiting.stage, Stage::AwaitingFlashFrame { .. }) {
                    return self.keep_waiting_for_free_gpu(waiting, now, deadline);
                }
                Some(waiting)
            }
            FlashRingPick::NotInRingYet if now >= deadline => {
                self.counters.flash.never_arrived += 1;
                self.finish(entry, StillOutcome::NotSecured(NoFrameReason::FlashFrameNeverArrived));
                None
            }
            FlashRingPick::NotInRingYet => {
                entry.stage = Stage::AwaitingFlashFrame { shot, deadline };
                Some(entry)
            }
            // Кольцо выключили посреди ожидания (поток закончился): кадр не появится, ждать срок незачем.
            FlashRingPick::RingDisabled => {
                self.counters.picks.no_frame_ring_disabled += 1;
                self.finish(entry, StillOutcome::NotSecured(NoFrameReason::RingDisabled));
                None
            }
        }
    }

    /// Слот копии или GPU заняты: запись ждёт, но срок не продлевается. Истёкший срок — тот же итог, что и у
    /// кадра, не дошедшего до кольца: запись не доживает до срока на занятом GPU.
    fn keep_waiting_for_free_gpu(
        &mut self,
        entry: StillEntry<G>,
        now: Instant,
        deadline: Instant,
    ) -> Option<StillEntry<G>> {
        if now < deadline {
            return Some(entry);
        }
        self.counters.flash.never_arrived += 1;
        self.finish(entry, StillOutcome::NotSecured(NoFrameReason::FlashFrameNeverArrived));
        None
    }

    /// Движок камеры сообщил, что вспышка снимка `id` не снята. Запись, ещё ждущая кадр, получает итог сразу;
    /// остальным (без вспышки, уже захваченным, неизвестным) — ничего: ответ опоздал.
    pub(super) fn apply_flash_failed(&mut self, id: StillId, failure: FlashStillFailure) {
        let position = self
            .queue
            .iter()
            .position(|entry| entry.plan.id == id && matches!(entry.stage, Stage::AwaitingFlashFrame { .. }));
        let Some(index) = position else {
            self.counters.flash.late_failure_reports += 1;
            return;
        };
        let entry = self.queue.remove(index);
        self.counters.flash.not_captured_by_camera += 1;
        self.finish(
            entry,
            StillOutcome::NotSecured(NoFrameReason::FlashNotCaptured(failure)),
        );
    }

    // ---- параметры кадра ----

    /// Параметры кадра со вспышкой (см. модуль). Нет плана или произведения кадра — «кадра нет» со счётчиком:
    /// подставлять единичную подтяжку нельзя (кадр проявился бы тёмным или пересвеченным молча).
    pub(super) fn flash_frame_params(
        &mut self,
        shot: FlashShotId,
        passport: &RawFramePassport,
    ) -> Result<FrameStillParams, NoFrameReason> {
        let Some(plan) = self.flash_facts.plan_for(shot) else {
            self.counters.flash.no_plan += 1;
            return Err(NoFrameReason::FrameUnusable);
        };
        let Some(frame_product) = frame_exposure_product(passport) else {
            self.counters.flash.no_exposure_product += 1;
            return Err(NoFrameReason::FrameUnusable);
        };
        // Сколько света надо добавить проявлением, чтобы средний серый сцены под вспышкой лёг в цель.
        let gain_ev = plan.mid_gray_product.ev_above(frame_product) as f32;
        let gain = self.residual_gain.quantize_ev(gain_ev);
        let white = match self.flash_facts.torch_white_for(&passport.stream) {
            Some(torch) => {
                self.counters.flash.white_from_torch_frame += 1;
                torch.white
            }
            None => {
                // Оценки по кадру с фонариком нет: белая видоискателя — худшее из доступного, но не молчаливое.
                let Some(viewfinder) = self.params_history.latest_of_stream(&passport.stream) else {
                    self.counters.params.missing += 1;
                    return Err(NoFrameReason::FrameUnusable);
                };
                self.counters.flash.white_fallback_no_torch_estimate += 1;
                tracing::warn!(target: super::LOG_TARGET, "нет оценки белой по кадру с фонариком: белая видоискателя");
                viewfinder.white
            }
        };
        Ok(FrameStillParams {
            stream: Arc::clone(&passport.stream),
            white,
            gain,
        })
    }

    // ---- проверка света ----

    /// Освещён ли захваченный кадр со вспышкой (см. `flash_lit`). `Some(причина)` — кадр отвергнут. Обычный
    /// снимок и нерешаемая проверка дают `None`.
    pub(super) fn reject_unlit_flash_frame(
        &mut self,
        raw: &G::Raw,
        facts: &StillFrameFacts,
        params: &IspParamsCache,
    ) -> Option<NoFrameReason> {
        let FrameChoice::FlashFrame { shot, .. } = facts.choice else {
            return None;
        };
        let verdict = self.judge_flash_frame_light(raw, facts, params, shot);
        match verdict {
            LitVerdict::Lit { .. } => self.counters.flash.lit_check_passed += 1,
            LitVerdict::NotLit { measured_share } => {
                tracing::warn!(target: super::LOG_TARGET, measured_share, "кадр со вспышкой не освещён");
                self.counters.flash.not_lit += 1;
                return Some(NoFrameReason::FlashFrameNotLit);
            }
            LitVerdict::Undecidable(LitUndecidable::PredictedShareTooLow) => {
                self.counters.flash.lit_check_skipped_low_predicted_share += 1;
            }
            LitVerdict::Undecidable(LitUndecidable::NoAmbientLevel) => {
                self.counters.flash.lit_check_skipped_no_ambient_level += 1;
            }
            LitVerdict::Undecidable(LitUndecidable::SamplesUnavailable) => {
                self.counters.flash.lit_check_skipped_samples_unavailable += 1;
            }
        }
        None
    }

    fn judge_flash_frame_light(
        &self,
        raw: &G::Raw,
        facts: &StillFrameFacts,
        params: &IspParamsCache,
        shot: FlashShotId,
    ) -> LitVerdict {
        let thresholds = LitThresholds {
            min_fraction_of_predicted: self.config.flash_lit_min_fraction_of_predicted,
            min_predicted_share: self.config.flash_lit_check_min_predicted_share,
        };
        // План есть: без него параметры кадра не собрались бы (`flash_frame_params`).
        let Some(plan) = self.flash_facts.plan_for(shot) else {
            return LitVerdict::Undecidable(LitUndecidable::SamplesUnavailable);
        };
        let predicted_share = plan.flash_share.value();
        // Дешёвый выход до чтения RAW: при малой доле вспышки проверка всё равно нерешаема.
        if predicted_share < f64::from(thresholds.min_predicted_share) {
            return LitVerdict::Undecidable(LitUndecidable::PredictedShareTooLow);
        }
        let ambient = self
            .flash_facts
            .ambient_level_of(&facts.stream, plan.ambient_frame)
            .map(|level| level.green_per_product);
        let (Some(gpu), Some(isp)) = (self.gpu.as_ref(), params.current()) else {
            return LitVerdict::Undecidable(LitUndecidable::SamplesUnavailable);
        };
        let Ok(samples) = gpu.raw_samples(raw) else {
            return LitVerdict::Undecidable(LitUndecidable::SamplesUnavailable);
        };
        let Some(frame_product) = ExposureProduct::from_result(
            facts.capture.result.exposure_time.unwrap_or_default(),
            facts.capture.result.sensitivity_iso.unwrap_or_default(),
        ) else {
            return LitVerdict::Undecidable(LitUndecidable::SamplesUnavailable);
        };
        // Кадрирование снимка — в координатах буфера; отсчёты начинаются с угла полезной области.
        let valid = facts.stream.valid_area;
        let area = camera_contract::geometry::PixelRect {
            left: facts.crop.left.saturating_sub(valid.left),
            top: facts.crop.top.saturating_sub(valid.top),
            size: facts.crop.size,
        };
        let sampling = GreenSampling {
            samples: &samples,
            pattern: isp.pattern,
            levels: &isp.levels,
            area,
            stride_cells: self.config.flash_lit_sample_stride_cells,
        };
        let Some(green) = mean_green_level(sampling) else {
            return LitVerdict::Undecidable(LitUndecidable::SamplesUnavailable);
        };
        // Обе стороны проверки — в отладочный лог: по ним видно, не горела ли вспышка или расходятся шкалы.
        tracing::debug!(
            target: "fasticamera::render::still",
            frame_green = green,
            frame_product = frame_product.seconds_iso(),
            frame_green_per_product = green / frame_product.seconds_iso(),
            ambient_green_per_product = ?ambient,
            predicted_share,
            ?area,
            "проверка освещённости кадра со вспышкой"
        );
        judge_lit(
            predicted_share,
            ambient,
            green / frame_product.seconds_iso(),
            thresholds,
        )
    }
}
