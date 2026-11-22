//! Потеря GPU снимков: исход работы на GPU неизвестен (fence не опросился или не просигналил в срок).
//!
//! # Правило crate
//! Объект, который GPU мог ещё читать или писать, при неизвестном исходе НЕ уничтожается — остаётся в учтённой
//! утечке (`live_objects`, `leaked_leases`). Поэтому потеря GPU — отдельный путь, а не обычный `finish`: запись
//! (`StillEntry`) с памятью GPU в стадии при выбрасывании запустила бы `Drop` этой памяти.
//!
//! # Что происходит
//! 1. GPU снимков объявляется потерянным БЕЗ ожидания ([`StillGpu::declare_lost`]: лизинг камеры в учтённую
//!    утечку, буфер копии, цель кодера, импорты — оставлены) и целиком выбрасывается (`forget`);
//! 2. каждая запись, чья стадия держит объекты GPU, получает итог (до `Secured` — `NotSecured(CaptureFailed)`,
//!    после — `Failed(Gpu)`), а объекты её стадии остаются в утечке;
//! 3. записи без объектов GPU (выбор кадра, ожидание копии) живут дальше: GPU для них создаётся заново
//!    (`ensure_gpu`), поэтому сбой одного fence не ломает все следующие затворы и не делает `cancel()` вечно «занято».

use super::engine::StillEngine;
use super::entry::{StillEntry, StillOutcome};
use super::gpu::StillGpu;
use still_contract::develop::DevelopFailure;
use still_contract::frame_choice::NoFrameReason;

impl<G: StillGpu> StillEngine<G> {
    /// Объявляет GPU потерянным; `culprit` — запись, на которой это обнаружено (уже вынута из очереди).
    pub(super) fn lose_gpu(&mut self, culprit: StillEntry<G>, reason: String) {
        tracing::error!(
            target: super::LOG_TARGET,
            %reason,
            "GPU снимков потерян: объекты проявления, копии и показа оставлены в учтённую утечку"
        );
        self.counters.gpu_lost += 1;
        if let Some(mut gpu) = self.gpu.take() {
            gpu.declare_lost();
            // Выброшенный GPU не должен выполнять `Drop` (он уничтожил бы то, что GPU мог ещё использовать).
            std::mem::forget(gpu);
        }
        self.finish_leaking_gpu_objects(culprit, &reason);
        let mut index = 0;
        while index < self.queue.len() {
            if self.queue[index].stage.holds_gpu_objects() {
                let entry = self.queue.remove(index);
                self.finish_leaking_gpu_objects(entry, &reason);
            } else {
                index += 1;
            }
        }
    }

    /// Итог записи, чьи объекты GPU надо оставить в утечке: объекты стадии забываются, итог доставляется.
    fn finish_leaking_gpu_objects(&mut self, mut entry: StillEntry<G>, reason: &str) {
        entry.leak_gpu_objects();
        let outcome = if entry.is_secured() {
            StillOutcome::Failed(DevelopFailure::Gpu(reason.to_owned()))
        } else {
            StillOutcome::NotSecured(NoFrameReason::CaptureFailed)
        };
        self.finish(entry, outcome);
    }
}
