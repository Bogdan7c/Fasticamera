//! Движок снимков без surface (сессия 14e, решения 146 и 150): уже снятое доводится до файла.
//!
//! # Что меняет пропажа surface
//! * Снимку, которому ещё нужен экран, делать нечего: выбор кадра (`Choosing`) доказывает «что было на экране» по
//!   журналу показов, а изъятие из кольца (`AwaitingCapture`) берёт параметры показанного кадра из истории. Обе
//!   ленты приходят только от видоискателя. Такие снимки получают честный `NotSecured(RendererStopped)`. Объектов
//!   GPU у них нет, поэтому итог выдаётся без ожидания GPU.
//! * Снимок с копией RAW в полёте (`Capturing`) и все захваченные (`AwaitingTarget` … `Presenting`) остаются в
//!   очереди. GPU снимков от surface не зависит: копия, полосы, мастерская и показ в поверхность кодера
//!   `HeifWriter` работают без экрана. Шаги им даёт поток рендера по своему таймеру (`DisplayBackend::still_workload`).
//!   Копию в полёте доводим, а не бросаем: бросить её честно можно только с ожиданием GPU (бюджет квитанции
//!   освобождения surface) или с утечкой буфера камеры.
//! * Отсоединение не ждёт GPU снимков вовсе. Это третье ожидание сверх двух, которые учитывает бюджет квитанции
//!   (`RenderLoopConfig::worst_case_wait_before_release_ms`), и раньше оно в бюджет не входило.
//!
//! # Память без экрана
//! Пока surface нет и очередь опустела, GPU снимков освобождается (буфер копии RAW, цели проявления, подготовленное
//! мастерской): фоновый процесс не держит лишние десятки МБ. Следующая лента видоискателя заново подготовит GPU
//! заранее (`prewarm_gpu`), как после запуска.

use still_contract::frame_choice::NoFrameReason;

use super::engine::StillEngine;
use super::entry::{Stage, StillOutcome};
use super::gpu::StillGpu;
use crate::still::StillWorkload;

/// Есть ли у видоискателя surface, с точки зрения движка снимков.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceLink {
    /// Видоискатель присоединён: приходят ленты показов и параметров.
    Attached,
    /// Surface нет (ещё не было или отпущена): лент нет, снимки двигает таймер потока рендера.
    Detached,
}

impl<G: StillGpu> StillEngine<G> {
    /// Surface отпущена. Не захваченные снимки, которым нужен экран, получают `NotSecured(RendererStopped)`, остальные
    /// продолжают путь без экрана. Ленты забываются: следующая surface — новый журнал показов. GPU не ждётся.
    pub fn on_detach(&mut self) {
        self.surface = SurfaceLink::Detached;
        self.for_each_in_stage(needs_display_to_continue, |engine, entry| {
            engine.counters.abandoned_before_capture += 1;
            engine.finish(entry, StillOutcome::NotSecured(NoFrameReason::RendererStopped));
            None
        });
        // Всё, что осталось, доводится без экрана (у каждого записи уже есть объекты GPU или своя копия RAW).
        self.counters.continued_without_surface += self.queue.len() as u64;
        if !self.queue.is_empty() {
            tracing::info!(
                target: super::LOG_TARGET,
                unfinished = self.queue.len(),
                "surface отпущена: захваченные снимки доводятся без экрана"
            );
        }
        self.present_log.restart();
        self.params_history.clear();
        self.current_stream = None;
        self.prewarm_failed_for = None;
        self.release_gpu_if_drained_without_surface();
    }

    /// Ждёт ли хоть один снимок итога (поток рендера будит себя по таймеру, пока ответ `Unfinished`).
    pub fn workload(&self) -> StillWorkload {
        if self.queue.is_empty() {
            StillWorkload::Idle
        } else {
            StillWorkload::Unfinished
        }
    }

    /// Без surface опустевшая очередь отпускает GPU снимков. С surface GPU держится готовым к следующему нажатию.
    pub(super) fn release_gpu_if_drained_without_surface(&mut self) {
        if self.surface != SurfaceLink::Detached || !self.queue.is_empty() {
            return;
        }
        if let Some(gpu) = self.gpu.take() {
            self.counters.gpu_released_without_surface += 1;
            self.retire_gpu(gpu);
        }
    }

    /// Разбирает GPU, у которого не осталось снимков: остановка (ограниченное ожидание отправленной работы), потом
    /// уничтожение. Не освободился за срок — намеренная учтённая утечка: память, в которую ещё может писать GPU, не
    /// уничтожается (соглашение crate). Остановка, а не простой `Drop`: отложенные цели кодера разбираются по
    /// правилам остановки.
    pub(super) fn retire_gpu(&mut self, mut gpu: G) {
        if gpu.abandon().gpu_idle {
            drop(gpu);
        } else {
            self.counters.gpu_leaked_on_abandon += 1;
            tracing::error!(
                target: super::LOG_TARGET,
                "GPU снимков не освободился при разборе: объекты намеренно оставлены в утечку"
            );
            std::mem::forget(gpu);
        }
    }
}

/// Стадии, которым для продолжения нужен экран (ленты видоискателя): только у них пропажа surface отнимает путь.
fn needs_display_to_continue<G: StillGpu>(stage: &Stage<G>) -> bool {
    matches!(stage, Stage::Choosing { .. } | Stage::AwaitingCapture(_))
}
