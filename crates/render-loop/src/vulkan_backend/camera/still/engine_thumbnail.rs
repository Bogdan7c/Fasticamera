//! Миниатюра снимка в движке (сессия 14g): забрать с проявленной картинки и отправить приёмнику РАНЬШЕ итога.
//!
//! Правила:
//! * миниатюра уходит приёмнику (`StillEventSink::deliver_thumbnail`) строго до итога снимка (`PrimaryRendered` /
//!   `PrimaryReadback`) и не больше одного раза на снимок (решение 154: картинка появляется, когда файл сохранён —
//!   итог приходит позже, но миниатюра к нему уже на руках);
//! * миниатюры может не быть (сбой уменьшения, цели не выделились): это счётчик `thumbnails_unavailable` и строка
//!   в логе с причиной. Итог снимка от этого не меняется — правило «ровно один итог» эта часть не трогает;
//! * потеря GPU на последней полосе даёт `Lost`/`Failed` ещё до `Done`: миниатюры нет и счётчики не двигаются, итог
//!   снимка идёт обычным путём потери GPU.

use isp_vulkan::photo::ThumbnailOutcome;
use still_contract::thumbnail::StillThumbnail;

use super::engine::StillEngine;
use super::entry::StillEntry;
use super::gpu::StillGpu;

impl<G: StillGpu> StillEngine<G> {
    /// Забирает миниатюру у проявленной картинки и отправляет её приёмнику записи (если она есть).
    pub(super) fn deliver_thumbnail_before_final(&mut self, entry: &mut StillEntry<G>, image: &mut G::Image) {
        let id = entry.plan.id;
        let Some(gpu) = self.gpu.as_ref() else {
            // GPU к этому моменту потерян: забрать нечего (картинка тоже уйдёт в утечку вместе с GPU).
            self.counters.thumbnails_unavailable += 1;
            tracing::warn!(target: super::LOG_TARGET, id = id.0, "миниатюры нет: GPU снимков потерян");
            return;
        };
        match gpu.take_thumbnail(image) {
            ThumbnailOutcome::Ready { size, rgba } => match StillThumbnail::new(id, size, rgba) {
                Ok(thumbnail) => {
                    if entry.deliver_thumbnail(thumbnail) {
                        self.counters.thumbnails_delivered += 1;
                    } else {
                        // Недостижимо (хук зовётся один раз и до итога), но молча терять миниатюру нельзя.
                        self.counters.thumbnails_unavailable += 1;
                        tracing::warn!(
                            target: super::LOG_TARGET,
                            id = id.0,
                            "миниатюра не отправлена: уже была или итог снимка уже выдан"
                        );
                    }
                }
                Err(mismatch) => {
                    self.counters.thumbnails_unavailable += 1;
                    tracing::warn!(
                        target: super::LOG_TARGET,
                        id = id.0,
                        ?mismatch,
                        "миниатюра не принята контрактом: снимок идёт без неё"
                    );
                }
            },
            ThumbnailOutcome::Unavailable(reason) => {
                self.counters.thumbnails_unavailable += 1;
                tracing::warn!(
                    target: super::LOG_TARGET,
                    id = id.0,
                    %reason,
                    "миниатюры нет: снимок идёт без неё"
                );
            }
        }
    }
}
