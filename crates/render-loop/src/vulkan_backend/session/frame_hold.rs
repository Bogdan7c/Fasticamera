//! Придержанная картинка swapchain: тик пропущен как «ничего не изменилось» (решение 214), а картинка уже выдана.
//!
//! # Зачем это нужно
//! Решение «рисовать ли тик» зависит от нового кадра камеры и списка рисования UI, а они известны только ПОСЛЕ
//! `vkAcquireNextImageKHR` (порядок «acquire → ожидание позднего захвата → опрос камеры» сохранён: иначе кадр
//! камеры, пришедший за время ожидания, опоздал бы на тик, а время acquire попало бы в оценку стоимости кадра).
//! Выданную картинку нельзя ни показать недорисованной (содержимое не определено), ни вернуть драйверу
//! (без `VK_EXT_swapchain_maintenance1` пары к acquire нет). Остаётся одно: придержать её и использовать на
//! следующем рисуемом тике вместо нового acquire.
//!
//! # Инварианты
//! * придержана не больше одной картинки, и пока она придержана, цель нового кадра не выдаст (`outstanding_frame`);
//! * картинка не рисовалась и не отправлялась: слот кольца свободен, его fence сигнален;
//! * придержанную картинку нельзя уничтожить молча: `Drop` кадра пишет ошибку «потерян». Перед пересозданием и
//!   отсоединением цели её отдают целиком на отказ ([`FrameHold::discard`]), после чего цель просит пересоздание
//!   (выданную картинку без maintenance1 вернуть нельзя — её swapchain отбрасывается).

use gpu_vulkan::swapchain::{DisplayFrame, DisplayTarget};

use super::LOG_TARGET;

/// Не более одной придержанной картинки swapchain.
#[derive(Debug, Default)]
pub(super) struct FrameHold {
    held: Option<DisplayFrame>,
}

impl FrameHold {
    /// Забирает придержанную картинку для следующего рисуемого тика (`None` — придержанной нет, нужен acquire).
    pub(super) fn take(&mut self) -> Option<DisplayFrame> {
        self.held.take()
    }

    /// Придерживает картинку пропущенного тика. Одновременно придержать две нельзя: цель не выдала бы вторую,
    /// пока первая не завершена; если это всё же случилось, прежняя отдаётся на отказ, а не теряется молча.
    pub(super) fn put(&mut self, frame: DisplayFrame, target: &mut DisplayTarget) {
        if let Some(previous) = self.held.replace(frame) {
            tracing::error!(target: LOG_TARGET, "придержана вторая картинка подряд: прежняя отдана цели на отказ");
            discard_frame(previous, target);
        }
    }

    /// Отдаёт придержанную картинку цели на отказ (перед пересозданием и отсоединением). Без неё — ничего.
    pub(super) fn discard(&mut self, target: &mut DisplayTarget) {
        if let Some(frame) = self.held.take() {
            discard_frame(frame, target);
        }
    }
}

/// Отказ от картинки: ошибка цели не прячется, а пишется в лог (картинка при этом всё равно завершена).
fn discard_frame(frame: DisplayFrame, target: &mut DisplayTarget) {
    if let Err(error) = target.discard_unpresented_frame(frame) {
        tracing::warn!(target: LOG_TARGET, %error, "отказ от придержанной картинки не принят целью");
    }
}
