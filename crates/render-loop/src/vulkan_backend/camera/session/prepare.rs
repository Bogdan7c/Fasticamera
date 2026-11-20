//! Решение о содержимом кадра: опрос видоискателя потока, параметры ISP, геометрия, импорт.
//!
//! Правило для нового кадра, который показывать нельзя (нет параметров, геометрия не влезла,
//! импорт не удался): лизинг возвращается нетронутым СРАЗУ (GPU его не касался), а кадр экрана
//! собирается так же, как без нового кадра.

use std::time::Instant;

use camera_contract::geometry::PixelRect;
use camera_contract::three_a::RequestOrigin;
use gpu_vulkan::raw_mosaic_preview::MosaicSource;
use gpu_vulkan::swapchain::DisplayColorSpace;
use gpu_vulkan::sync::{WaitSemaphore, import_wait_fence};
use isp_model::exposure::ExposureGain;
use isp_vulkan::viewfinder::{ViewfinderGeometry, gamut_for_display};
use raw_frame_stream::{RawFrameLease, ViewfinderPoll};

use camera_contract::zoom::FramedCrop;
use shell_contract::clock::MonotonicNanos;

use super::super::continuity::{ExposureReadiness, PictureMeta, ProcessedFrame, Screen, StreamEvent};
use super::super::exposure::{ExposureFrameFacts, ExposureFramePlan, ExposureSlotFrame};
use super::super::isp_params::ParamsOutcome;
use super::super::passport::preview_params;
use super::super::statistics_schedule::StatisticsDecision;
use super::transition::TransitionFrame;
use super::{CameraSession, Content, LOG_TARGET, NewFrameError, Prepared};
use crate::viewfinder::{AwbFrameIdentity, FrameCropReport, ViewfinderPainter};
use crate::vulkan_backend::camera::SubmissionSerial;
use crate::vulkan_backend::camera::lease::FrameChange;
use crate::vulkan_backend::camera::pipelines::IspPresentation;

impl CameraSession {
    /// Решает, что рисовать в этом кадре.
    ///
    /// `now` — время показа кадра (шкала vsync): по нему машина непрерывности отсчитывает сроки удержания.
    pub(super) fn prepare_content(
        &mut self,
        serial: SubmissionSerial,
        color_space: DisplayColorSpace,
        now: MonotonicNanos,
    ) -> Prepared {
        let painter = self.settings.painter;
        // Единственный опрос потока за кадр показа: при буфере видоискателя каждый опрос расходует
        // кадр очереди, лишний «съел» бы кадр следующего vsync. Пропущенный ради схода тик (решение 54)
        // сюда не доходит вовсе — его кадр покажется следующим.
        match (self.frames.poll_for_vsync(Instant::now()), painter) {
            // Кадр серии с брекетингом: у кадров серии разные выдержки, на экране они мигали бы, а AWB по
            // ним ошибся бы. Возвращается камере нетронутым, считается отдельно (не «пропуск» в «ровно один раз»).
            (ViewfinderPoll::NewFrame(lease), _) if is_bracket_frame(&lease) => {
                lease.release_unused();
                self.viewfinder.counters_mut().bracket_frames_skipped += 1;
                match painter {
                    ViewfinderPainter::Isp => self.prepare_isp_without_new_frame(now, StreamEvent::Unpresentable),
                    ViewfinderPainter::DebugMosaic => self.prepare_mosaic_redraw(serial),
                }
            }
            (ViewfinderPoll::NewFrame(lease), ViewfinderPainter::Isp) => {
                self.prepare_isp_new(lease, serial, color_space, now)
            }
            (ViewfinderPoll::NewFrame(lease), ViewfinderPainter::DebugMosaic) => self.prepare_mosaic_new(lease, serial),
            (ViewfinderPoll::NoNewFrame, ViewfinderPainter::Isp) => {
                self.prepare_isp_without_new_frame(now, StreamEvent::Quiet)
            }
            (ViewfinderPoll::NoNewFrame, ViewfinderPainter::DebugMosaic) => self.prepare_mosaic_redraw(serial),
            (ViewfinderPoll::StreamInactive, _) => {
                // Следующий кадр — уже другой поток: подсказки и параметры ISP прежнего потока ему
                // чужие (решение 42), история шумодава стенда V4 его не смешает.
                self.params.forget_stream();
                self.white.forget_stream();
                if let Some(exposure) = self.exposure.as_mut() {
                    exposure.forget_stream();
                }
                self.bench.note_stream_inactive();
                // Продукт (ISP) удерживает последний показанный кадр вместо чёрного (решения 110, 113);
                // отладочная мозаика — стенд замера, непрерывности у неё нет.
                let (content, transition) = match painter {
                    ViewfinderPainter::Isp => {
                        let transition = self.plan_recompose_transition(now, StreamEvent::Inactive);
                        (content_of(&transition), Some(transition))
                    }
                    ViewfinderPainter::DebugMosaic => (Content::Black, None),
                };
                Prepared {
                    content,
                    change: FrameChange::Clear,
                    new_lease: None,
                    camera_wait: None,
                    stream_inactive: true,
                    transition,
                }
            }
        }
    }

    // ---- ISP видоискателя ----

    /// Новый кадр через ISP: параметры → геометрия → импорт и acquire-fence. Сабмит поглотит кадр.
    fn prepare_isp_new(
        &mut self,
        lease: RawFrameLease,
        serial: SubmissionSerial,
        color_space: DisplayColorSpace,
        now: MonotonicNanos,
    ) -> Prepared {
        let gamut = gamut_for_display(color_space);
        // Наш AWB решает до параметров: какая белая идёт в ISP, показывать ли кадр, нужна ли статистика.
        let plan = self.white.plan_frame(lease.passport());
        // Наш AE (если включён): остаточная подтяжка этого кадра и его потребность в статистике. Подтяжка входит
        // в параметры кадра, поэтому решается до них.
        let exposure_plan = self
            .exposure
            .as_mut()
            .map(|exposure| exposure.plan_frame(lease.passport()));
        let exposure_gain = exposure_plan.map_or(ExposureGain::UNITY, |exposure_plan| exposure_plan.gain);
        let decision = self
            .params
            .prepare_with_exposure_gain(lease.passport(), plan.white, exposure_gain, gamut);
        self.viewfinder.note_params(&decision);
        if let ParamsOutcome::Unavailable(reason) = &decision.outcome {
            tracing::debug!(target: LOG_TARGET, ?reason, "параметров ISP нет: кадр не показывается");
            lease.release_unused();
            return self.prepare_isp_without_new_frame(now, StreamEvent::Unpresentable);
        }
        let stream = &lease.passport().stream;
        // Кадрирование — для камеры ЭТОГО кадра (по паспорту его потока), а не «текущей»: кадр старой камеры,
        // ещё идущий через ISP во время переключения, не получает кадрирование новой (нет скачка на кадр).
        let framed = self.settings.crop.framed_for(&stream.camera);
        let geometry = match ViewfinderGeometry::from_crop(stream.valid_area, &framed.crop, stream.sensor_orientation) {
            Ok(geometry) => geometry,
            Err(error) => {
                self.viewfinder.counters_mut().frames_rejected_geometry += 1;
                tracing::warn!(target: LOG_TARGET, %error, "геометрия кадра не подходит ISP: кадр возвращён нетронутым");
                lease.release_unused();
                return self.prepare_isp_without_new_frame(now, StreamEvent::Unpresentable);
            }
        };
        match self.import_with_fence(&lease, serial) {
            Ok((source, wait)) => {
                self.stats.camera_frames_imported += 1;
                let identity = frame_identity(&lease);
                let statistics = StatisticsDecision::combine(
                    plan.statistics,
                    exposure_plan.map(|exposure_plan| exposure_plan.statistics),
                );
                let exposure = self.exposure_slot_frame(exposure_plan, &lease, geometry.crop);
                // Стенд V4 (если включён) читает тот же кадр в том же сабмите: параметры Menon из тех
                // же входов, нагрузка — по полезной области.
                let headroom =
                    self.bench
                        .prepare_frame(lease.passport(), plan.white, gamut, self.viewfinder.headroom_mut());
                let transition = self.plan_new_frame_transition(&lease, plan, &framed, &geometry, now);
                Prepared {
                    transition: Some(transition),
                    content: Content::IspNew {
                        source,
                        geometry,
                        identity,
                        plan,
                        statistics,
                        exposure,
                        headroom,
                    },
                    change: FrameChange::Consume,
                    new_lease: Some(lease),
                    camera_wait: Some(wait),
                    stream_inactive: false,
                }
            }
            Err(error) => {
                self.stats.camera_frame_failures += 1;
                tracing::warn!(target: LOG_TARGET, %error, "кадр камеры не подготовлен: возвращён нетронутым");
                lease.release_unused();
                self.prepare_isp_without_new_frame(now, StreamEvent::Unpresentable)
            }
        }
    }

    /// Решение непрерывности для нового кадра: поток, готовность цвета (AWB) и яркости (AE), направление камеры.
    fn plan_new_frame_transition(
        &self,
        lease: &RawFrameLease,
        plan: crate::vulkan_backend::camera::white_balance::FramePlan,
        framed: &FramedCrop,
        geometry: &ViewfinderGeometry,
        now: MonotonicNanos,
    ) -> TransitionFrame {
        let passport = lease.passport();
        let facing = self.settings.crop.facing_of(&passport.stream.camera);
        // Вендорский AE (раннера нет): измерять нечего, ждать яркости незачем.
        let exposure = self.exposure.as_ref().map_or(ExposureReadiness::Ready, |runner| {
            runner.reveal_readiness(
                passport,
                self.continuity_config.reveal_exposure,
                self.continuity_config.max_exposure_error_ev,
            )
        });
        let event = StreamEvent::Processed(ProcessedFrame {
            stream: plan.stream,
            color_known: plan.presentation == IspPresentation::Show,
            exposure,
        });
        let picture = PictureMeta {
            stream: plan.stream,
            shown_ratio: framed.shown_ratio,
            mirror: framed.mirror,
            facing,
        };
        let crop = FrameCropReport {
            camera: passport.stream.camera.clone(),
            resolved: geometry.crop,
            framed: *framed,
        };
        self.plan_transition(now, event, Some(picture), Some(crop))
    }

    /// Что слот помнит о кадре для замера AE: поток и факты кадра (выдержка×ISO результата, веса яркости
    /// параметров, кадрирование). `None` — AE выключен или параметров кадра нет (недостижимо после `prepare`).
    fn exposure_slot_frame(
        &self,
        exposure_plan: Option<ExposureFramePlan>,
        lease: &RawFrameLease,
        crop: PixelRect,
    ) -> Option<ExposureSlotFrame> {
        let (exposure_plan, params) = (exposure_plan?, self.params.current()?);
        Some(ExposureSlotFrame {
            stream: exposure_plan.stream,
            facts: ExposureFrameFacts::of_frame(lease.passport(), params, crop),
        })
    }

    /// Нового кадра для ISP нет: перекомпоновка прежнего изображения (или чёрный, если его нет).
    /// Кадр отладочной мозаики, оставшийся на экране после смены рисовальщика, возвращается этим сабмитом.
    fn prepare_isp_without_new_frame(&mut self, now: MonotonicNanos, event: StreamEvent) -> Prepared {
        let change = if self.ledger.displayed().is_some() {
            FrameChange::Clear
        } else {
            FrameChange::Keep
        };
        // Что на экране — решает машина непрерывности: живое изображение, удержанный кадр или чёрный.
        let transition = self.plan_recompose_transition(now, event);
        Prepared {
            content: content_of(&transition),
            change,
            new_lease: None,
            camera_wait: None,
            stream_inactive: false,
            transition: Some(transition),
        }
    }

    // ---- отладочная мозаика (05) ----

    /// Новый кадр отладочной мозаики: он останется на экране до замены.
    fn prepare_mosaic_new(&mut self, lease: RawFrameLease, serial: SubmissionSerial) -> Prepared {
        let prepared = preview_params(lease.passport())
            .map_err(|error| NewFrameError(error.to_string()))
            .and_then(|params| {
                self.import_with_fence(&lease, serial)
                    .map(|(source, wait)| (source, params, wait))
            });
        match prepared {
            Ok((source, params, wait)) => {
                self.stats.camera_frames_imported += 1;
                Prepared {
                    content: Content::Mosaic { source, params },
                    change: FrameChange::Replace,
                    new_lease: Some(lease),
                    camera_wait: Some(wait),
                    stream_inactive: false,
                    transition: None,
                }
            }
            Err(error) => {
                self.stats.camera_frame_failures += 1;
                tracing::warn!(target: LOG_TARGET, %error, "кадр камеры не подготовлен: возвращён нетронутым");
                lease.release_unused();
                self.prepare_mosaic_redraw(serial)
            }
        }
    }

    /// Перерисовка показанного кадра мозаики (или чёрный, если показывать нечего).
    fn prepare_mosaic_redraw(&mut self, serial: SubmissionSerial) -> Prepared {
        let content = match self.ledger.displayed() {
            Some(lease) => match preview_params(lease.passport())
                .map_err(|error| error.to_string())
                .and_then(|params| {
                    self.importer
                        .import_frame(lease, serial)
                        .map(|source| Content::Mosaic { source, params })
                        .map_err(|error| error.to_string())
                }) {
                Ok(content) => content,
                Err(error) => {
                    self.stats.camera_frame_failures += 1;
                    tracing::warn!(target: LOG_TARGET, %error, "перерисовать показанный кадр не удалось: чёрный кадр");
                    Content::Black
                }
            },
            None => Content::Black,
        };
        Prepared {
            content,
            change: FrameChange::Keep,
            new_lease: None,
            camera_wait: None,
            stream_inactive: false,
            transition: None,
        }
    }

    // ---- общее ----

    /// Импорт буфера и семафор ожидания acquire-fence для НОВОГО кадра.
    fn import_with_fence(
        &mut self,
        lease: &RawFrameLease,
        serial: SubmissionSerial,
    ) -> Result<(MosaicSource, WaitSemaphore), NewFrameError> {
        let source = self.importer.import_frame(lease, serial).map_err(NewFrameError::from)?;
        let fence = lease
            .acquire_fence()
            .map_err(|error| NewFrameError(format!("acquire-fence кадра: {error}")))?;
        let wait = import_wait_fence(&self.context, fence)
            .map_err(|error| NewFrameError(format!("импорт acquire-fence в семафор: {}", error.reason)))?;
        Ok((source, wait))
    }
}

/// Содержимое кадра без нового кадра камеры по решению непрерывности: чёрный или перекомпоновка (живая или
/// удержанная — какая именно, решает `plan` при записи).
fn content_of(transition: &TransitionFrame) -> Content {
    match transition.plan.screen {
        Screen::Black => Content::Black,
        Screen::Live | Screen::Held => Content::Recompose,
    }
}

/// Кадр из серии с брекетингом (`RequestOrigin::Burst`). Кадр без метаданных серией не считается: его
/// происхождение неизвестно, поведение прежнее (решение 42).
fn is_bracket_frame(lease: &RawFrameLease) -> bool {
    lease
        .passport()
        .capture
        .details()
        .is_some_and(|details| matches!(details.result.three_a.origin, RequestOrigin::Burst { .. }))
}

/// Чей кадр (для привязки статистики AWB).
fn frame_identity(lease: &RawFrameLease) -> AwbFrameIdentity {
    let passport = lease.passport();
    AwbFrameIdentity {
        sequence: passport.sequence.0,
        sensor_timestamp_nanos: passport.sensor_timestamp.as_nanos(),
        frame_number: passport.capture.details().map(|details| details.result.frame_number.0),
    }
}
