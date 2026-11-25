//! Лента снимков из сессии показа: что лежало на экране под каждым показом (вынесено из `session` ради размера).
//!
//! Сессия лишь записывает факты в [`super::super::still::StillFeedRecorder`]; кто и как их читает, она не знает.

use gpu_vulkan::swapchain::{PresentOutcome as GpuPresentOutcome, SwapchainError};
use raw_frame_stream::RawFrameLease;

use super::{CameraSession, Content, Recorded};
use crate::vulkan_backend::camera::continuity::Composed;
use crate::vulkan_backend::camera::still::{ShownNow, StillFeed};

impl CameraSession {
    /// Забирает накопленное для движка снимков (лента становится пустой).
    pub fn take_still_feed(&mut self) -> StillFeed {
        // Факты о свете для кадра со вспышки (14d): планы из указаний AE, уровень света сцены.
        if let Some(exposure) = self.exposure.as_mut() {
            for plan in exposure.drain_flash_plans() {
                self.still_feed.note_flash_plan(plan);
            }
            for level in exposure.drain_ambient_levels() {
                self.still_feed.note_ambient_level(level);
            }
        }
        self.still_feed.take()
    }

    /// Цепочка показа пересоздана: номера показов начались заново.
    pub fn note_presents_restarted(&mut self) {
        self.still_feed.note_presents_restarted();
        // Цепочка новая: на её картинках ещё ничего не показано, прежний кадр сравнения больше не на экране.
        self.unchanged.invalidate();
    }

    /// Записывает, что легло на экран под принятым показом. Живым считается только изображение ISP
    /// (`Composed::Live`): удержанная картинка смены объектива и чёрный кадр — не кадр камеры.
    pub(super) fn note_still_present(
        &mut self,
        content: &Content,
        recorded: Recorded,
        composed: Composed,
        new_lease: Option<&RawFrameLease>,
        outcome: &Result<GpuPresentOutcome, SwapchainError>,
    ) {
        let present_id = match outcome {
            Ok(GpuPresentOutcome::Presented { present_id } | GpuPresentOutcome::Suboptimal { present_id }) => {
                *present_id
            }
            Ok(GpuPresentOutcome::NeedsRecreate) | Err(_) => return,
        };
        let live = composed == Composed::Live;
        let shown = match (recorded, content) {
            (
                Recorded::IspNew {
                    processed: true,
                    shown: true,
                },
                Content::IspNew { .. },
            ) if live => new_lease.map_or(ShownNow::NotLive, |lease| {
                ShownNow::NewLiveFrame(lease.passport().sensor_timestamp)
            }),
            (Recorded::Recompose { shown: true }, _) if live => ShownNow::RecomposedLive,
            _ => ShownNow::NotLive,
        };
        self.still_feed.note_present(present_id, shown);
    }
}
