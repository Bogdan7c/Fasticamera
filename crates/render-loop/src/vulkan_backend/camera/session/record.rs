//! Запись команд кадра по решению `prepare` и плану владения.
//!
//! Мозаика и чёрный кадр проверяют всё ДО записи команд: их отказ — `Err`, командный буфер чист,
//! новый лизинг можно вернуть нетронутым. Кадр ISP не отказывает никогда: приём и возврат кадра
//! записаны всегда, отказ ISP или компоновщика заменяется чёрным и считается в диагностике.

use gpu_vulkan::error::GpuError;
use gpu_vulkan::raw_mosaic_preview::{FrameHandoff, PreviewFrame, SourceAcquire};
use gpu_vulkan::swapchain::DisplayFrame;

use super::super::ownership::{AcquireNew, OwnershipPlan, ReleaseDisplayed};
use super::super::pipelines::{AcquiredSourceReader, CameraPipelines, IspFrameRecording, NewIspFrame, ScreenContent};
use super::slot::effective_stages;
use super::{CameraSession, Content, LOG_TARGET, Recorded};

impl CameraSession {
    /// Записывает содержимое кадра в командный буфер `frame`.
    pub(super) fn record_content(
        &mut self,
        pipelines: &mut CameraPipelines,
        frame: &DisplayFrame,
        content: &Content,
        plan: OwnershipPlan,
    ) -> Result<Recorded, GpuError> {
        // Возврат прежнего кадра мозаики — в сабмите, который его заменяет или убирает с экрана.
        let release_displayed = match plan.release_displayed {
            ReleaseDisplayed::Yes => self.displayed_source,
            ReleaseDisplayed::No => None,
        };
        let placement = self.settings.placement;
        match content {
            Content::Mosaic { source, params } => {
                let handoff = FrameHandoff {
                    acquire_source: match plan.acquire_new {
                        AcquireNew::Yes => SourceAcquire::BeforeRead,
                        AcquireNew::No => SourceAcquire::AlreadyAcquired,
                    },
                    release_previous: release_displayed,
                };
                pipelines.preview.record(
                    frame.command_buffer(),
                    frame.pattern_target(),
                    &PreviewFrame {
                        source,
                        params,
                        transform: frame.transform(),
                        frame_slot: frame.ring_slot(),
                        handoff,
                    },
                )?;
                Ok(Recorded::Mosaic)
            }
            Content::Black => {
                pipelines
                    .preview
                    .record_black(frame.command_buffer(), frame.pattern_target(), release_displayed)?;
                Ok(Recorded::Black)
            }
            Content::Recompose => {
                let recording = pipelines.record_recompose(frame, release_displayed, placement);
                let shown = self.account_recording(pipelines, &recording);
                Ok(Recorded::Recompose { shown })
            }
            Content::IspNew {
                source,
                geometry,
                identity,
                plan,
                statistics,
                exposure: _,
                headroom,
            } => {
                let Some(params) = self.params.current() else {
                    // Недостижимо: `prepare` пропускает кадр к ISP только с параметрами. Кадр не принят
                    // у камеры (барьеров нет), сабмит вернёт его по своему fence — это безопасно.
                    tracing::error!(target: LOG_TARGET, "кадр ISP без параметров: чёрный кадр");
                    self.viewfinder.counters_mut().isp_record_failures += 1;
                    pipelines.preview.record_black(
                        frame.command_buffer(),
                        frame.pattern_target(),
                        release_displayed,
                    )?;
                    return Ok(Recorded::IspNew {
                        processed: false,
                        shown: false,
                    });
                };
                // Стенд V4 читает тот же кадр между ISP и возвратом камере (итог — после сабмита).
                let mut bench_recorder = headroom.map(|bench_frame| self.bench.recorder(bench_frame));
                let recording = pipelines.record_new_frame(
                    frame,
                    &NewIspFrame {
                        source,
                        params,
                        geometry: *geometry,
                        stages: effective_stages(self.settings.stages, statistics.gpu),
                        frame_number: identity.sequence,
                        presentation: plan.presentation,
                    },
                    bench_recorder
                        .as_mut()
                        .map(|recorder| recorder as &mut dyn AcquiredSourceReader),
                    release_displayed,
                    placement,
                );
                let processed = recording.isp.is_ok();
                let shown = self.account_recording(pipelines, &recording);
                Ok(Recorded::IspNew { processed, shown })
            }
        }
    }

    /// Учитывает отказы записи кадра видоискателя; возвращает, попало ли изображение на экран.
    fn account_recording(&mut self, pipelines: &CameraPipelines, recording: &IspFrameRecording) -> bool {
        let counters = self.viewfinder.counters_mut();
        counters.gpu_timing_failures += recording.timing_failures;
        if let Err(error) = &recording.isp {
            counters.isp_record_failures += 1;
            counters.last_isp_failure = Some(error.to_string());
            tracing::warn!(target: LOG_TARGET, %error, "ISP видоискателя не записан: кадр возвращён камере, на экране чёрный");
        }
        let shown = match &recording.screen {
            ScreenContent::Viewfinder => true,
            ScreenContent::Black { compose_failure } => {
                if compose_failure.is_some() {
                    counters.compose_failures += 1;
                }
                false
            }
        };
        if recording.isp.is_ok()
            && let Some(image) = pipelines.isp().image()
        {
            self.viewfinder.note_isp_image(image);
        }
        shown
    }
}
