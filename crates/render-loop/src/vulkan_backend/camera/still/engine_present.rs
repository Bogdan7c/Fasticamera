//! Показ проявленной картинки в поверхность кодера (HEIF): запуск, опрос, срок.
//!
//! Картинка (`Stage::Presenting { _image }`) живёт до конца показа: GPU читает её копией (blit). Поэтому запись
//! со стадией `Presenting` нельзя просто завершить, пока исход GPU неизвестен: `finish` уничтожил бы картинку под
//! работающим GPU. Если исход неизвестен (сорвался опрос fence или истёк срок), вызывается
//! [`StillEngine::lose_gpu`]: он сначала объявляет GPU потерянным, а картинку оставляет в учтённой утечке.

use std::time::{Duration, Instant};

use still_contract::develop::DevelopFailure;

use super::engine::StillEngine;
use super::entry::{Stage, StillEntry, StillOutcome};
use super::gpu::{PresentProgress, StillGpu};

impl<G: StillGpu> StillEngine<G> {
    /// Запускает показ картинки в поверхность кодера и переводит запись в `Presenting`.
    pub(super) fn start_present(
        &mut self,
        mut entry: StillEntry<G>,
        surface: shell_contract::surface::NativeSurface,
        image: G::Image,
    ) -> Option<StillEntry<G>> {
        let Some(gpu) = self.gpu.as_mut() else {
            self.finish(
                entry,
                StillOutcome::Failed(DevelopFailure::Gpu("GPU снимков потерян".to_owned())),
            );
            return None;
        };
        match gpu.present_start(&image, surface) {
            Ok(()) => {
                self.counters.encoder_presents_started += 1;
                let deadline =
                    self.clock.now() + Duration::from_millis(u64::from(self.config.encoder_present_timeout_ms));
                entry.stage = Stage::Presenting {
                    deadline,
                    _image: image,
                };
                Some(entry)
            }
            Err(refusal) => {
                self.finish(entry, StillOutcome::Failed(DevelopFailure::Gpu(refusal.0)));
                None
            }
        }
    }

    /// Шаг показа: освободившийся слот отдаёт следующей готовой картинке, затем опрашивает идущий показ.
    pub(super) fn step_present(&mut self, now: Instant) {
        // Освободился слот показа — следующая готовая картинка.
        let present_busy = self.gpu.as_ref().is_some_and(StillGpu::present_in_flight);
        if !present_busy
            && let Some((index, entry)) = self.take_first_in_stage(|stage| matches!(stage, Stage::Developed { .. }))
        {
            let mut entry = entry;
            let entry = match entry.take_stage() {
                Stage::Developed { surface, image } => self.start_present(entry, surface, image),
                other => {
                    entry.stage = other;
                    Some(entry)
                }
            };
            self.put_back(index, entry);
        }
        let Some((index, entry)) = self.take_first_in_stage(|stage| matches!(stage, Stage::Presenting { .. })) else {
            return;
        };
        let progress = match self.gpu.as_mut() {
            Some(gpu) => gpu.present_poll(),
            None => PresentProgress::Failed("GPU снимков потерян".to_owned()),
        };
        let deadline = match &entry.stage {
            Stage::Presenting { deadline, .. } => *deadline,
            _ => now,
        };
        match progress {
            PresentProgress::Done => {
                self.counters.encoder_presents_completed += 1;
                self.finish(entry, StillOutcome::Rendered);
            }
            // GPU закончил копирование (доказано fence-ом): картинку можно уничтожать вместе с записью.
            PresentProgress::Failed(error) => self.finish(entry, StillOutcome::Failed(DevelopFailure::Gpu(error))),
            PresentProgress::Idle => self.finish(
                entry,
                StillOutcome::Failed(DevelopFailure::Gpu("показ в кодер пропал у GPU".to_owned())),
            ),
            PresentProgress::Lost(error) => {
                tracing::error!(target: super::LOG_TARGET, %error, "опрос показа не удался: исход на GPU неизвестен");
                self.lose_gpu(entry, error);
            }
            PresentProgress::Pending if now < deadline => self.put_back(index, Some(entry)),
            PresentProgress::Pending => {
                // Копия или показ не кончаются: отменить их нельзя, ждать больше нечего. Картинка остаётся в
                // утечке (GPU мог ещё её читать), GPU снимков пересоздаётся для следующего снимка.
                tracing::error!(target: super::LOG_TARGET, "показ в кодер не завершился в срок");
                self.counters.gpu_lost_by_deadline += 1;
                self.lose_gpu(entry, "показ в кодер не завершился за отведённое время".to_owned());
            }
        }
    }
}
