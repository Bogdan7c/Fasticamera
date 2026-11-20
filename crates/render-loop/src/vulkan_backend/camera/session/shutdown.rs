//! Освобождение сессии показа при отсоединении surface (вынесено из `session` без изменения поведения).

use gpu_vulkan::raw_mosaic_preview::RawMosaicPreview;
use gpu_vulkan::swapchain::DisplayTarget;

use super::{CameraSession, LOG_TARGET};
use crate::camera_diagnostics::CameraStreamDiagnostics;
use crate::vulkan_backend::camera::viewfinder_stats::ViewfinderStats;

/// Итог освобождения сессии.
pub struct CameraShutdown {
    /// Счётчики сессии (в них же — сколько лизингов возвращено после простоя и сколько утекло).
    pub diagnostics: CameraStreamDiagnostics,
    /// Статистика видоискателя возвращается владельцу (бэкенду) — она копится за его жизнь.
    pub viewfinder: ViewfinderStats,
}

impl CameraSession {
    /// Освобождение сессии при отсоединении surface. Возвращает итоговые счётчики.
    ///
    /// GPU дождался ⇒ все лизинги возвращаются камере с сигнальным fence, импорты уничтожаются.
    /// GPU не освободился вовремя ⇒ лизинги НАМЕРЕННО выбрасываются (учтённая утечка, решение 43), импортёр и
    /// семафоры тоже оставляются в утечке: GPU может их ещё использовать.
    pub fn shutdown(mut self, display: &DisplayTarget) -> CameraShutdown {
        let gpu_wait = display.wait_for_submitted_frames();
        match &gpu_wait {
            Ok(()) => {
                self.tracker.note_gpu_idle();
                // Кадр мозаики на экране ещё у нас: отдельным возвратом владения передаём его камере.
                if let Some(source) = self.displayed_source.take()
                    && let Err(error) = RawMosaicPreview::release_source_blocking(&self.context, &source)
                {
                    tracing::warn!(target: LOG_TARGET, %error, "возврат владения показанным кадром не записан");
                }
                let released = self.ledger.release_all_after_gpu_idle();
                self.stats.leases_released_after_idle += released as u64;
                self.importer.release_all_imports_gpu_idle();
                self.waits_in_flight.clear();
                self.bench.release_all_gpu_idle(self.viewfinder.headroom_mut());
            }
            Err(error) => {
                let leaked = self.ledger.leak_all();
                self.stats.leases_leaked_on_teardown += leaked as u64;
                tracing::error!(
                    target: LOG_TARGET,
                    %error,
                    leaked,
                    "GPU не освободился: лизинги кадров камеры намеренно выброшены (учтённая утечка), импорты оставлены"
                );
            }
        }
        let diagnostics = self.diagnostics();
        let Self {
            importer,
            export,
            waits_in_flight,
            viewfinder,
            bench,
            ..
        } = self;
        if gpu_wait.is_err() {
            // Импортёр, семафор экспорта, семафоры ожидания и объекты стенда V4 может ещё использовать
            // GPU: не уничтожаем.
            std::mem::forget(importer);
            std::mem::forget(export);
            std::mem::forget(waits_in_flight);
            std::mem::forget(bench);
        }
        CameraShutdown {
            diagnostics,
            viewfinder,
        }
    }
}
