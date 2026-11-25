//! Сбор итогов слота кольца после его fence: время GPU и статистика кадра (AE, лента, белая светодиода), очередь времён кадров.
//!
//! Вынесено из `session.rs` без изменения поведения (его размер упёрся в предел модуля при подключении AE);
//! статистика слота теперь уходит каждому потребителю по решению [`StatisticsDecision`] кадра.

use std::time::Duration;

use gpu_vulkan::gpu_timing::TimestampCollect;

use super::{CameraPipelines, CameraSession, LOG_TARGET, SlotFrame};
use crate::backend::FrameGpuTiming;
use crate::vulkan_backend::camera::ui_state::UiOverlayState;

/// Сколько времён кадра на GPU держать до вычерпывания потоком. Поток забирает их каждый кадр
/// (обычно 0–1 штука), так что 64 — заведомый запас; дальше — вытеснение со счётчиком.
const MAX_PENDING_GPU_TIMINGS: usize = 64;

impl CameraSession {
    /// Итоги слота после его fence: время GPU и статистика AWB (публикуется с кадром, который её дал).
    pub(super) fn collect_slot(&mut self, pipelines: &mut CameraPipelines, slot: usize, ui: &mut UiOverlayState) {
        let collected = pipelines.collect_slot(slot);
        self.bench.collect_slot(slot, self.viewfinder.headroom_mut());
        let slot_frame = self.slot_frames.get_mut(slot).and_then(Option::take);
        match collected.isp {
            Ok(report) => {
                self.viewfinder.note_isp_dispatch_time(&report.isp_gpu_time);
                if let Some(statistics) = report.awb_statistics {
                    match slot_frame {
                        Some(slot_frame) => {
                            // Сначала потребители (по ссылке), затем публикация: статистика уходит и ленте.
                            self.route_statistics(&slot_frame, &statistics);
                            self.awb_feed.publish(slot_frame.identity, statistics);
                            self.viewfinder.counters_mut().statistics_published += 1;
                        }
                        None => self.viewfinder.counters_mut().statistics_discarded += 1,
                    }
                }
            }
            Err(error) => {
                self.viewfinder.counters_mut().gpu_timing_failures += 1;
                tracing::warn!(target: LOG_TARGET, %error, slot, "итоги слота ISP не собраны");
            }
        }
        if let Err(error) = collected.compositor {
            tracing::warn!(target: LOG_TARGET, %error, slot, "набор дескрипторов компоновщика не освобождён");
        }
        if let Err(error) = collected.ui {
            tracing::warn!(target: LOG_TARGET, %error, slot, "слот рендера UI не освобождён");
        }
        // Приборы освободили слот внутри `pipelines.collect_slot`: их приращения уходят в статистику видоискателя.
        self.note_scope_events(&pipelines.take_scope_events());
        let slot_frame_index = self.slot_frame_indices.get(slot).copied().flatten();
        match collected.frame {
            Ok(frame_time) => {
                self.viewfinder.note_frame_time(&frame_time);
                ui.note_frame_time(&frame_time);
                if let (Some(frame_index), TimestampCollect::Ready(spans)) = (slot_frame_index, &frame_time) {
                    self.push_gpu_timing(FrameGpuTiming {
                        frame_index,
                        gpu_duration: Duration::from_nanos(spans.total_ns),
                    });
                }
            }
            Err(error) => {
                self.viewfinder.counters_mut().gpu_timing_failures += 1;
                tracing::warn!(target: LOG_TARGET, %error, slot, "метки кадра видоискателя не собраны");
            }
        }
    }

    /// Учитывает приращения цветовых приборов, которые накопила цепочка показа.
    pub(crate) fn note_scope_events(&mut self, events: &crate::viewfinder::ScopeEvents) {
        if !events.is_empty() {
            self.viewfinder.note_scope_events(events);
        }
    }

    /// Отдаёт статистику кадра тем потребителям, которые её просили (решение расписания кадра): AE — любой
    /// посчитанный кадр.
    ///
    /// Кадр, освещённый светодиодом (14d), питает видоискатель НЕ ТАК: AE меряет его для расчёта вспышки, но
    /// внутри runner не отдаёт контроллеру. Белая видоискателя статистикой не питается вовсе (её ведёт владелец
    /// белого), а белая кадра со вспышкой — постоянный цвет светодиода из плана движка.
    fn route_statistics(&mut self, slot_frame: &SlotFrame, statistics: &isp_model::statistics::AwbStatistics) {
        let light = slot_frame.light;
        if !light.feeds_viewfinder_smoothing() {
            self.viewfinder.counters_mut().led_lit_frames_withheld_from_smoothing += 1;
        }
        if slot_frame.statistics.ae_receives()
            && let (Some(exposure), Some(frame)) = (self.exposure.as_mut(), slot_frame.exposure.as_ref())
        {
            exposure.ingest_statistics(frame.stream, slot_frame.identity, &frame.facts, statistics);
        }
    }

    /// Запоминает номер кадра потока, чьи команды записаны в слот.
    pub(super) fn note_slot_frame_index(&mut self, slot: usize, frame_index: Option<u64>) {
        if self.slot_frame_indices.len() <= slot {
            self.slot_frame_indices.resize(slot + 1, None);
        }
        self.slot_frame_indices[slot] = frame_index;
    }

    /// Кладёт время кадра на GPU в очередь для потока. Поток вычерпывает её каждый кадр; если
    /// он почему-то не вычерпывает, самые старые замеры вытесняются (очередь ограничена).
    fn push_gpu_timing(&mut self, timing: FrameGpuTiming) {
        if self.completed_gpu_timings.len() >= MAX_PENDING_GPU_TIMINGS {
            self.completed_gpu_timings.remove(0);
            self.viewfinder.counters_mut().gpu_timings_overflowed += 1;
        }
        self.completed_gpu_timings.push(timing);
    }

    /// Вычерпывает собранные времена кадров на GPU (по возрастанию номера кадра).
    pub fn drain_frame_gpu_timings(&mut self) -> Vec<FrameGpuTiming> {
        std::mem::take(&mut self.completed_gpu_timings)
    }

    /// Запоминает, чей кадр ISP записал в слот (`None` — статистику слота публиковать нельзя).
    pub(super) fn note_slot_frame(&mut self, slot: usize, frame: Option<SlotFrame>) {
        if self.slot_frames.len() <= slot {
            self.slot_frames.resize(slot + 1, None);
        }
        self.slot_frames[slot] = frame;
    }
}
