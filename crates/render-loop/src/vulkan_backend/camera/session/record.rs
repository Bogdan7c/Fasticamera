//! Запись команд кадра по решению `prepare` и плану владения.
//!
//! Мозаика и чёрный кадр проверяют всё ДО записи команд: их отказ — `Err`, командный буфер чист,
//! новый лизинг можно вернуть нетронутым. Кадр ISP не отказывает никогда: приём и возврат кадра
//! записаны всегда, отказ ISP или компоновщика заменяется чёрным и считается в диагностике.

use gpu_vulkan::error::GpuError;
use gpu_vulkan::raw_mosaic_preview::{FrameHandoff, MosaicSource, PreviewFrame, SourceAcquire};
use gpu_vulkan::swapchain::DisplayFrame;

use super::super::ownership::{AcquireNew, OwnershipPlan, ReleaseDisplayed};
use super::super::pipelines::{
    AcquiredSourceReader, CameraPipelines, FrameTarget, IspFrameRecording, NewIspFrame, ScreenContent,
};
use super::slot::effective_stages;
use super::transition::{TransitionFrame, backdrop_of};
use super::{CameraSession, Content, LOG_TARGET, Recorded, SubmissionSerial};
use crate::viewfinder::ViewfinderPainter;
use crate::vulkan_backend::camera::continuity::Composed;
use crate::vulkan_backend::camera::pipelines::{ShowFrameAs, mirror_of};
use crate::vulkan_backend::camera::ui_state::UiOverlayFrame;
use ui_draw::frame_source::BackdropContinuity;

/// Что записано в кадре: содержимое для счётчиков и то, что реально попало на экран.
pub(super) struct RecordedFrame {
    pub recorded: Recorded,
    pub composed: Composed,
}

/// Решения `prepare` для записи кадра: что рисовать, кто владеет кадрами и что решила непрерывность.
pub(super) struct FrameDecision<'a> {
    /// Содержимое кадра.
    pub content: &'a Content,
    /// План владения буферами камеры.
    pub ownership: OwnershipPlan,
    /// Решение непрерывности (`None` у отладочной мозаики): по нему начинается удержание и выбирается, что лечь
    /// на экран.
    pub transition: Option<&'a TransitionFrame>,
}

impl CameraSession {
    /// Записывает содержимое кадра в командный буфер `frame` по решениям `decision`.
    pub(super) fn record_content(
        &mut self,
        pipelines: &mut CameraPipelines,
        frame: &DisplayFrame,
        decision: &FrameDecision<'_>,
        serial: SubmissionSerial,
        ui: &mut UiOverlayFrame<'_>,
    ) -> Result<RecordedFrame, GpuError> {
        let FrameDecision {
            content,
            ownership: plan,
            transition,
        } = *decision;
        // Удержание начинается ДО записи кадра нового потока: ISP перезаписал бы живую картинку, и последний
        // показанный кадр пропал бы. Отцепление — смена владельца картинки, команд не пишет.
        if let Some(held) = transition.and_then(|transition| transition.plan.begins_hold)
            && let Err(error) = pipelines.hold_live_image(held)
        {
            tracing::warn!(target: LOG_TARGET, %error, "удержание без картинки: кадры будут чёрными");
            self.viewfinder.continuity_mut().counters_mut().holds_without_image += 1;
        }
        // Желание экрана о приборах действует на КАЖДОМ кадре и любом содержимом (в том числе после «скрыть» на кадрах
        // перекомпоновки и чёрных): так выведенные объекты приборов освобождаются по слотам без новых кадров камеры.
        // Считаются приборы только пока их есть кому показывать: рисовальщик ISP (мозаика приборов не знает) и
        // установленный источник UI — то же состояние, что решает рисование UI (`ui.state.is_active()` ниже).
        pipelines.set_scope_demand(self.settings.effective_scope_demand(ui.state.source_presence()));
        let show = transition.map_or(ShowFrameAs::Live, TransitionFrame::show);
        let continuity = backdrop_of(transition, BackdropContinuity::Live);
        // Возврат прежнего кадра мозаики — в сабмите, который его заменяет или убирает с экрана.
        let release_displayed = match plan.release_displayed {
            ReleaseDisplayed::Yes => self.displayed_source,
            ReleaseDisplayed::No => None,
        };
        let placement = self.settings.effective_placement();
        // Дальше пишем в цель через узкий `FrameTarget` (тот же, что строят тесты на offscreen-цели).
        let target = FrameTarget::of(frame);
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
                Ok(RecordedFrame {
                    recorded: Recorded::Mosaic,
                    composed: Composed::Live,
                })
            }
            Content::Black => {
                let continuity = backdrop_of(transition, BackdropContinuity::NoCameraFrame);
                self.record_black_content(pipelines, frame, release_displayed, serial, ui, continuity)?;
                Ok(RecordedFrame {
                    recorded: Recorded::Black,
                    composed: Composed::Black,
                })
            }
            Content::Recompose => {
                let recording = pipelines.record_recompose_as(&target, release_displayed, placement, show);
                self.record_ui_overlay(pipelines, &target, &recording, serial, ui, continuity);
                let composed = self.account_recording(pipelines, &recording);
                Ok(RecordedFrame {
                    recorded: Recorded::Recompose {
                        shown: composed == Composed::Live,
                    },
                    composed,
                })
            }
            Content::IspNew {
                source,
                geometry,
                identity,
                light: _,
                plan: _,
                statistics,
                exposure: _,
                headroom,
            } => {
                let Some(params) = self.params.current() else {
                    // Недостижимо: `prepare` пропускает кадр к ISP только с параметрами. Кадр не принят
                    // у камеры (барьеров нет), сабмит вернёт его по своему fence — это безопасно.
                    tracing::error!(target: LOG_TARGET, "кадр ISP без параметров: чёрный кадр");
                    self.viewfinder.counters_mut().isp_record_failures += 1;
                    let continuity = backdrop_of(transition, BackdropContinuity::NoCameraFrame);
                    self.record_black_content(pipelines, frame, release_displayed, serial, ui, continuity)?;
                    return Ok(RecordedFrame {
                        recorded: Recorded::IspNew {
                            processed: false,
                            shown: false,
                        },
                        composed: Composed::Black,
                    });
                };
                // Стенд V4 читает тот же кадр между ISP и возвратом камере (итог — после сабмита).
                let mut bench_recorder = headroom.map(|bench_frame| self.bench.recorder(bench_frame));
                let recording = pipelines.record_new_frame(
                    &target,
                    &NewIspFrame {
                        submission: gpu_vulkan::backdrop::SubmissionSerial(serial.0),
                        source,
                        params,
                        geometry: *geometry,
                        stages: effective_stages(self.settings.stages, statistics.gpu),
                        frame_number: identity.sequence,
                        show,
                        mirror: transition
                            .and_then(|transition| transition.picture)
                            .map_or(Default::default(), |picture| mirror_of(picture.mirror)),
                    },
                    bench_recorder
                        .as_mut()
                        .map(|recorder| recorder as &mut dyn AcquiredSourceReader),
                    release_displayed,
                    placement,
                );
                let processed = recording.isp.is_ok();
                self.record_ui_overlay(pipelines, &target, &recording, serial, ui, continuity);
                let composed = self.account_recording(pipelines, &recording);
                Ok(RecordedFrame {
                    recorded: Recorded::IspNew {
                        processed,
                        shown: composed == Composed::Live,
                    },
                    composed,
                })
            }
        }
    }

    /// Чёрный экран без изображения камеры (потока нет, кадра ещё не было, параметров нет).
    ///
    /// Продукт (рисовальщик ISP) с установленным источником UI рисует UI поверх чёрного — экран камеры не
    /// пропадает, когда камера закрыта или ещё не дала кадр (сессия 12). Отладочная мозаика (05) и тестовая
    /// картинка остаются без UI: это стенды замера, а не продукт, а чистый кадр мозаики нужен для A/B-замера.
    fn record_black_content(
        &mut self,
        pipelines: &mut CameraPipelines,
        frame: &DisplayFrame,
        release_displayed: Option<MosaicSource>,
        serial: SubmissionSerial,
        ui: &mut UiOverlayFrame<'_>,
        continuity: BackdropContinuity,
    ) -> Result<(), GpuError> {
        let target = &FrameTarget::of(frame);
        let draws_ui = self.settings.painter == ViewfinderPainter::Isp && ui.state.is_active();
        if !draws_ui {
            return pipelines
                .preview
                .record_black(frame.command_buffer(), frame.pattern_target(), release_displayed);
        }
        let recording = pipelines.record_black_frame(target, release_displayed)?;
        self.viewfinder.counters_mut().gpu_timing_failures += recording.timing_failures;
        self.record_ui_overlay(pipelines, target, &recording, serial, ui, continuity);
        Ok(())
    }

    /// UI поверх только что записанной компоновки (без источника — ничего). Отказы записи меток времени UI
    /// идут в общий счётчик отказов замера кадра; сам UI считает свои отказы в `UiOverlayState`.
    fn record_ui_overlay(
        &mut self,
        pipelines: &mut CameraPipelines,
        frame: &FrameTarget,
        recording: &IspFrameRecording,
        serial: SubmissionSerial,
        ui: &mut UiOverlayFrame<'_>,
        continuity: BackdropContinuity,
    ) {
        let timing_failures = pipelines.record_ui_overlay(frame, ui, &recording.screen, serial, continuity);
        self.viewfinder.counters_mut().gpu_timing_failures += timing_failures;
    }

    /// Учитывает отказы записи кадра видоискателя; возвращает, что реально попало на экран.
    fn account_recording(&mut self, pipelines: &CameraPipelines, recording: &IspFrameRecording) -> Composed {
        let counters = self.viewfinder.counters_mut();
        counters.gpu_timing_failures += recording.timing_failures;
        if let Err(error) = &recording.isp {
            counters.isp_record_failures += 1;
            counters.last_isp_failure = Some(error.to_string());
            tracing::warn!(target: LOG_TARGET, %error, "ISP видоискателя не записан: кадр возвращён камере, на экране чёрный");
        }
        let composed = match &recording.screen {
            ScreenContent::Viewfinder { .. } => Composed::Live,
            ScreenContent::Held { .. } => Composed::Held,
            ScreenContent::Black { compose_failure } => {
                if compose_failure.is_some() {
                    counters.compose_failures += 1;
                }
                Composed::Black
            }
        };
        if recording.isp.is_ok()
            && let Some(image) = pipelines.isp().image()
        {
            self.viewfinder.note_isp_image(image);
        }
        composed
    }
}
