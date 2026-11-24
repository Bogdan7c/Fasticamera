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
use super::transition::TransitionFrame;
use super::{CameraSession, Content, LOG_TARGET, Recorded, SubmissionSerial};

use crate::vulkan_backend::camera::continuity::Composed;
use crate::vulkan_backend::camera::pipelines::{ShowFrameAs, UiTick, mirror_of};
use crate::vulkan_backend::camera::ui_state::UiOverlayFrame;

/// Что записано в кадре: содержимое для счётчиков и то, что реально попало на экран.
pub(super) struct RecordedFrame {
    pub recorded: Recorded,
    pub composed: Composed,
    /// UI тика записан полностью (или UI в этом кадре и не требовался): пиксели кадра — компоновка + список UI.
    pub ui_complete: bool,
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
    /// UI тика, уже подготовленный ДО записи (источник спрошен один раз, решение 214): запись только дописывает его.
    pub ui_tick: &'a UiTick,
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
            ui_tick,
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
                    // Мозаике UI не положен: «полный» кадр — только мозаика.
                    ui_complete: true,
                })
            }
            Content::Black => {
                let ui_complete =
                    self.record_black_content(pipelines, frame, release_displayed, serial, ui, ui_tick)?;
                Ok(RecordedFrame {
                    recorded: Recorded::Black,
                    composed: Composed::Black,
                    ui_complete,
                })
            }
            Content::Recompose => {
                let recording = pipelines.record_recompose_as(&target, release_displayed, placement, show);
                let ui_complete = self.record_ui_overlay(pipelines, &target, &recording, serial, ui, ui_tick);
                let composed = self.account_recording(pipelines, &recording);
                Ok(RecordedFrame {
                    recorded: Recorded::Recompose {
                        shown: composed == Composed::Live,
                    },
                    composed,
                    ui_complete,
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
                    // UI этого тика подготовлен как для живого кадра (фон `Live`): путь недостижим, поэтому отдельного
                    // запроса к источнику ради «чёрного» фона не делаем — источник обязан вызываться раз на тик.
                    let ui_complete =
                        self.record_black_content(pipelines, frame, release_displayed, serial, ui, ui_tick)?;
                    return Ok(RecordedFrame {
                        recorded: Recorded::IspNew {
                            processed: false,
                            shown: false,
                        },
                        composed: Composed::Black,
                        ui_complete,
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
                let ui_complete = self.record_ui_overlay(pipelines, &target, &recording, serial, ui, ui_tick);
                let composed = self.account_recording(pipelines, &recording);
                Ok(RecordedFrame {
                    recorded: Recorded::IspNew {
                        processed,
                        shown: composed == Composed::Live,
                    },
                    composed,
                    ui_complete,
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
        ui_tick: &UiTick,
    ) -> Result<bool, GpuError> {
        let target = &FrameTarget::of(frame);
        // `Inactive` — и источника нет, и под отладочной мозаикой UI не положен (решает `prepare_ui_tick`).
        let draws_ui = !matches!(ui_tick, UiTick::Inactive);
        if !draws_ui {
            pipelines
                .preview
                .record_black(frame.command_buffer(), frame.pattern_target(), release_displayed)?;
            return Ok(true);
        }
        let recording = pipelines.record_black_frame(target, release_displayed)?;
        self.viewfinder.counters_mut().gpu_timing_failures += recording.timing_failures;
        Ok(self.record_ui_overlay(pipelines, target, &recording, serial, ui, ui_tick))
    }

    /// UI поверх только что записанной компоновки: пишет в GPU список, подготовленный ДО записи кадра (без
    /// источника — ничего). Отказы записи меток времени UI идут в общий счётчик отказов замера кадра; сам UI
    /// считает свои отказы в `UiOverlayState`. Возвращает, записан ли UI тика полностью (или его и не требовалось):
    /// от этого зависит, годится ли кадр как эталон «ничего не изменилось».
    fn record_ui_overlay(
        &mut self,
        pipelines: &mut CameraPipelines,
        frame: &FrameTarget,
        recording: &IspFrameRecording,
        serial: SubmissionSerial,
        ui: &mut UiOverlayFrame<'_>,
        ui_tick: &UiTick,
    ) -> bool {
        match ui_tick {
            UiTick::Inactive => true,
            // Подавленный замером A/B UI не записывается (источник свой кадр получил): кадр не эталон.
            UiTick::Suppressed | UiTick::Unavailable => false,
            UiTick::Ready(_) => {
                let outcome = pipelines.record_prepared_ui(frame, ui, ui_tick, &recording.screen, serial);
                self.viewfinder.counters_mut().gpu_timing_failures += outcome.timing_failures;
                outcome.drawn
            }
        }
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
