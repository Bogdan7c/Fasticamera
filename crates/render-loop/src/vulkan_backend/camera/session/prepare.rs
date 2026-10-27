//! Решение о содержимом кадра: опрос видоискателя потока, параметры ISP, геометрия, импорт.
//!
//! Правило для нового кадра, который показывать нельзя (нет параметров, геометрия не влезла,
//! импорт не удался): лизинг возвращается нетронутым СРАЗУ (GPU его не касался), а кадр экрана
//! собирается так же, как без нового кадра.

use std::time::Instant;

use gpu_vulkan::raw_mosaic_preview::MosaicSource;
use gpu_vulkan::swapchain::DisplayColorSpace;
use gpu_vulkan::sync::{WaitSemaphore, import_wait_fence};
use isp_vulkan::viewfinder::{ViewfinderGeometry, gamut_for_display};
use raw_frame_stream::{RawFrameLease, ViewfinderPoll};

use super::super::isp_params::ParamsOutcome;
use super::super::passport::preview_params;
use super::{CameraSession, Content, LOG_TARGET, NewFrameError, Prepared};
use crate::viewfinder::{AwbFrameIdentity, ViewfinderPainter};
use crate::vulkan_backend::camera::SubmissionSerial;
use crate::vulkan_backend::camera::lease::FrameChange;

impl CameraSession {
    /// Решает, что рисовать в этом кадре.
    pub(super) fn prepare_content(&mut self, serial: SubmissionSerial, color_space: DisplayColorSpace) -> Prepared {
        let painter = self.settings.painter;
        // Единственный опрос потока за кадр показа: при буфере видоискателя каждый опрос расходует
        // кадр очереди, лишний «съел» бы кадр следующего vsync. Пропущенный ради схода тик (решение 54)
        // сюда не доходит вовсе — его кадр покажется следующим.
        match (self.frames.poll_for_vsync(Instant::now()), painter) {
            (ViewfinderPoll::NewFrame(lease), ViewfinderPainter::Isp) => {
                self.prepare_isp_new(lease, serial, color_space)
            }
            (ViewfinderPoll::NewFrame(lease), ViewfinderPainter::DebugMosaic) => self.prepare_mosaic_new(lease, serial),
            (ViewfinderPoll::NoNewFrame, ViewfinderPainter::Isp) => self.prepare_isp_without_new_frame(),
            (ViewfinderPoll::NoNewFrame, ViewfinderPainter::DebugMosaic) => self.prepare_mosaic_redraw(serial),
            (ViewfinderPoll::StreamInactive, _) => {
                // Следующий кадр — уже другой поток: подсказки и параметры ISP прежнего потока ему
                // чужие (решение 42), история шумодава стенда V4 его не смешает.
                self.params.forget_stream();
                self.bench.note_stream_inactive();
                Prepared {
                    content: Content::Black,
                    change: FrameChange::Clear,
                    new_lease: None,
                    camera_wait: None,
                    stream_inactive: true,
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
    ) -> Prepared {
        let gamut = gamut_for_display(color_space);
        let decision = self.params.prepare(lease.passport(), gamut);
        self.viewfinder.note_params(&decision);
        if let ParamsOutcome::Unavailable(reason) = &decision.outcome {
            tracing::debug!(target: LOG_TARGET, ?reason, "параметров ISP нет: кадр не показывается");
            lease.release_unused();
            return self.prepare_isp_without_new_frame();
        }
        let stream = &lease.passport().stream;
        let geometry = match ViewfinderGeometry::from_crop(
            stream.valid_area,
            &self.settings.crop,
            stream.sensor_orientation,
        ) {
            Ok(geometry) => geometry,
            Err(error) => {
                self.viewfinder.counters_mut().frames_rejected_geometry += 1;
                tracing::warn!(target: LOG_TARGET, %error, "геометрия кадра не подходит ISP: кадр возвращён нетронутым");
                lease.release_unused();
                return self.prepare_isp_without_new_frame();
            }
        };
        match self.import_with_fence(&lease, serial) {
            Ok((source, wait)) => {
                self.stats.camera_frames_imported += 1;
                let identity = frame_identity(&lease);
                // Стенд V4 (если включён) читает тот же кадр в том же сабмите: параметры Menon из тех
                // же входов, нагрузка — по полезной области.
                let headroom = self
                    .bench
                    .prepare_frame(lease.passport(), gamut, self.viewfinder.headroom_mut());
                Prepared {
                    content: Content::IspNew {
                        source,
                        geometry,
                        identity,
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
                self.prepare_isp_without_new_frame()
            }
        }
    }

    /// Нового кадра для ISP нет: перекомпоновка прежнего изображения (или чёрный, если его нет).
    /// Кадр отладочной мозаики, оставшийся на экране после смены рисовальщика, возвращается этим сабмитом.
    fn prepare_isp_without_new_frame(&mut self) -> Prepared {
        let change = if self.ledger.displayed().is_some() {
            FrameChange::Clear
        } else {
            FrameChange::Keep
        };
        Prepared {
            content: if self.isp_image_shown {
                Content::Recompose
            } else {
                Content::Black
            },
            change,
            new_lease: None,
            camera_wait: None,
            stream_inactive: false,
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

/// Чей кадр (для привязки статистики AWB).
fn frame_identity(lease: &RawFrameLease) -> AwbFrameIdentity {
    let passport = lease.passport();
    AwbFrameIdentity {
        sequence: passport.sequence.0,
        sensor_timestamp_nanos: passport.sensor_timestamp.as_nanos(),
        frame_number: passport.capture.details().map(|details| details.result.frame_number.0),
    }
}
