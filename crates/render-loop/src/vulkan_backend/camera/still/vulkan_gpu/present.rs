//! Показ картинки снимка в поверхность кодера на стороне GPU: задание в полёте и цели, чей показ нельзя отследить.
//!
//! # Поток рендера не ждёт
//! Цель кодера ([`EncoderInputTarget`]) нельзя уничтожать, пока движок показа не закончил с её swapchain. Раньше
//! разбор цели ждал показ (`wait_presents_done`, при отсутствии `VK_EXT_swapchain_maintenance1` — `vkDeviceWaitIdle`
//! без срока) прямо в `present_poll` на потоке рендера. Теперь:
//!
//! ```text
//! CopyRunning ──(fence копии)──▶ try_detach ──▶ Detached            → Done
//!                                    │  └─────▶ PresentsPending ──(следующий кадр: опрос fence-ов показа)──┐
//!                                    │                  ▲──────────────────────────────────────────────────┘
//!                                    └────────▶ DeviceWideWaitRequired → отложена (`retired`), показ считается
//!                                               законченным для снимка, цель разберёт остановка GPU
//! ```
//!
//! `DeviceWideWaitRequired` — устройство без `VK_EXT_swapchain_maintenance1` (или показ с неизвестным исходом):
//! узнать окончание показа опросом нельзя, а ждать на потоке рендера нельзя. Рамка кадра в кодер уже передана
//! (`vkQueuePresentKHR` вернулся, копия по fence завершена), цель лишь не разобрана, поэтому снимок получает
//! `Rendered`, а цель остаётся в `retired` до остановки GPU снимков ([`PresentChannel::settle_after_idle`]: там GPU
//! простаивает, ограниченное ожидание допустимо) либо, сверх `retired_present_capacity`, — в учтённой утечке.

use gpu_vulkan::context::GpuContext;
use gpu_vulkan::encoder_input::{DetachProgress, EncoderAttachError, EncoderDetachError, EncoderInputTarget};
use gpu_vulkan::encoder_input::{PresentOnceError, SubmittedButNotPresented};
use gpu_vulkan::one_shot_queue::{CompletedWork, OneShotSubmitter, SubmittedWork, WorkStatus};
use gpu_vulkan::rgba8_image::Rgba8StorageImage;
use gpu_vulkan::swapchain::PresentationSurface;
use shell_contract::surface::NativeSurface;

use super::super::gpu::{PresentProgress, PresentRefusal};

/// Что известно о показе в задании.
enum PresentStage {
    /// Копия картинки в swapchain ещё не подтверждена fence-ом.
    CopyRunning(SubmittedWork),
    /// Копия закончена (доказательство в руках), ждём fence-ы показа.
    PresentsPending(CompletedWork),
}

/// Показ в поверхность кодера в полёте.
struct PresentJob {
    target: EncoderInputTarget,
    stage: PresentStage,
    /// Сбой показа после отправки копии: дожидаемся GPU и сообщаем итог.
    failure: Option<String>,
    /// ПК-стенд: окно держится до конца показа (цель работает на `Headless`). Поле после `target`: окно живёт
    /// дольше цели.
    _held_window: Option<NativeSurface>,
}

/// Чем доказывать завершение работы цели при остановке.
enum TeardownProof {
    /// Копия завершена этим доказательством.
    Copy(CompletedWork),
    /// В цель ничего не отправляли (сорвалась отправка после acquire).
    NeverSubmitted,
}

/// Цель, чей показ нельзя отследить опросом: ждёт остановки GPU снимков.
struct RetiredTarget {
    target: EncoderInputTarget,
    proof: TeardownProof,
    _held_window: Option<NativeSurface>,
}

/// Канал показа в кодер: одно задание в полёте и отложенные цели.
pub(super) struct PresentChannel {
    job: Option<PresentJob>,
    retired: Vec<RetiredTarget>,
    /// Сколько отложенных целей держать; сверх — самая старая в учтённой утечке.
    retired_capacity: usize,
    /// Шов отказов: цели создаются «без fence-ов показа» (как на устройстве без `VK_EXT_swapchain_maintenance1`).
    #[cfg(feature = "fault-injection")]
    untracked_presents: bool,
}

impl PresentChannel {
    pub(super) fn new(retired_capacity: usize) -> Self {
        Self {
            job: None,
            retired: Vec::new(),
            retired_capacity,
            #[cfg(feature = "fault-injection")]
            untracked_presents: false,
        }
    }

    /// Шов отказов: следующие цели не отслеживают показ fence-ами (устройство без `VK_EXT_swapchain_maintenance1`).
    #[cfg(all(test, feature = "fault-injection"))]
    pub(super) fn inject_untracked_presents(&mut self) {
        self.untracked_presents = true;
    }

    /// Сколько целей отложено до остановки.
    #[cfg(all(test, feature = "fault-injection"))]
    pub(super) fn retired_len(&self) -> usize {
        self.retired.len()
    }

    /// Идёт ли показ (задание в полёте).
    pub(super) fn in_flight(&self) -> bool {
        self.job.is_some()
    }

    /// Запускает показ `image` в `surface`. Поверхность принимается в любом случае.
    pub(super) fn start(
        &mut self,
        context: &GpuContext,
        submitter: &mut OneShotSubmitter,
        image: &Rgba8StorageImage,
        surface: NativeSurface,
    ) -> Result<(), PresentRefusal> {
        if self.job.is_some() {
            return Err(PresentRefusal("показ в кодер уже идёт".to_owned()));
        }
        let (presentation, held_window) = presentation_for_encoder(surface);
        let extent = image.extent();
        let mut target = match EncoderInputTarget::attach(context, presentation, extent) {
            Ok(target) => target,
            Err(EncoderAttachError { error, surface }) => {
                release_encoder_surface(surface);
                return Err(PresentRefusal(format!("поверхность кодера: {error}")));
            }
        };
        #[cfg(feature = "fault-injection")]
        if self.untracked_presents {
            target.force_device_wide_wait_for_test();
        }
        match target.present_once(submitter, image) {
            Ok(work) => {
                self.job = Some(PresentJob {
                    target,
                    stage: PresentStage::CopyRunning(work),
                    failure: None,
                    _held_window: held_window,
                });
                Ok(())
            }
            Err(PresentOnceError::NotSubmitted(error)) => {
                // Ничего не ушло на GPU: цель разбирается без ожидания, поверхность отпускается.
                self.dispose_unsubmitted(target, held_window);
                Err(PresentRefusal(format!("показ не начат: {error}")))
            }
            Err(PresentOnceError::SubmittedButNotPresented(sent)) => {
                // Копия ушла на GPU: работу терять нельзя — дождёмся её и сообщим сбой.
                let SubmittedButNotPresented { work, error } = *sent;
                self.job = Some(PresentJob {
                    target,
                    stage: PresentStage::CopyRunning(work),
                    failure: Some(format!("показ сорвался: {error}")),
                    _held_window: held_window,
                });
                Ok(())
            }
        }
    }

    /// Опрос показа без ожидания.
    pub(super) fn poll(&mut self, submitter: &mut OneShotSubmitter) -> PresentProgress {
        let Some(job) = self.job.take() else {
            return PresentProgress::Idle;
        };
        let PresentJob {
            target,
            stage,
            failure,
            _held_window: held_window,
        } = job;
        let proof = match stage {
            PresentStage::CopyRunning(work) => match submitter.poll(&work) {
                WorkStatus::Pending => {
                    self.job = Some(PresentJob {
                        target,
                        stage: PresentStage::CopyRunning(work),
                        failure,
                        _held_window: held_window,
                    });
                    return PresentProgress::Pending;
                }
                WorkStatus::Completed(proof) => proof,
                WorkStatus::Failed(error) => {
                    // Исход копии неизвестен: цель — учтённая утечка (`Drop` цели не уничтожает объекты показа).
                    drop(target);
                    return PresentProgress::Lost(format!("опрос показа: {error:?}"));
                }
            },
            PresentStage::PresentsPending(proof) => proof,
        };
        match target.try_detach(&proof) {
            Ok(DetachProgress::Detached(surface)) => {
                // Поверхность кодера отпускается здесь: кодер дальше завершает запись сам.
                release_encoder_surface(surface);
                drop(held_window);
                failure.map_or(PresentProgress::Done, PresentProgress::Failed)
            }
            Ok(DetachProgress::PresentsPending(target)) => {
                self.job = Some(PresentJob {
                    target: *target,
                    stage: PresentStage::PresentsPending(proof),
                    failure,
                    _held_window: held_window,
                });
                PresentProgress::Pending
            }
            Ok(DetachProgress::DeviceWideWaitRequired(target)) => {
                self.retire(RetiredTarget {
                    target: *target,
                    proof: TeardownProof::Copy(proof),
                    _held_window: held_window,
                });
                failure.map_or(PresentProgress::Done, PresentProgress::Failed)
            }
            Err(EncoderDetachError::ProofMismatch { target } | EncoderDetachError::ProofRequired { target }) => {
                // Нарушение внутреннего протокола (доказательство не той работы): цель в учтённой утечке.
                drop(target);
                PresentProgress::Failed("разбор цели кодера: доказательство не той работы".to_owned())
            }
            Err(EncoderDetachError::GpuNotIdle { error, surface }) => {
                // Опрос fence-ов показа не удался (потеря устройства): объекты цели уже оставлены в утечке.
                release_encoder_surface(surface);
                PresentProgress::Lost(format!("опрос показа в кодер: {error}"))
            }
            Err(EncoderDetachError::AlreadyReleased) => PresentProgress::Failed("цель кодера уже разобрана".to_owned()),
        }
    }

    /// GPU потерян: цель и отложенные цели остаются в учтённой утечке (`Drop` цели не уничтожает объекты показа).
    pub(super) fn declare_lost(&mut self) {
        self.job = None;
        self.retired.clear();
    }

    /// Остановка, GPU не освободился: всё, что он мог ещё использовать, остаётся в утечке.
    pub(super) fn abandon_leaking(&mut self) {
        self.declare_lost();
    }

    /// Остановка после `drain`: GPU простаивает, цели разбираются (ограниченное ожидание допустимо).
    pub(super) fn settle_after_idle(&mut self, submitter: &mut OneShotSubmitter) {
        if let Some(job) = self.job.take() {
            let PresentJob { target, stage, .. } = job;
            let proof = match stage {
                PresentStage::CopyRunning(work) => match submitter.poll(&work) {
                    WorkStatus::Completed(proof) => Some(proof),
                    WorkStatus::Pending | WorkStatus::Failed(_) => None,
                },
                PresentStage::PresentsPending(proof) => Some(proof),
            };
            match proof {
                Some(proof) => detach_blocking(target, TeardownProof::Copy(proof)),
                None => drop(target),
            }
        }
        for retired in std::mem::take(&mut self.retired) {
            detach_blocking(retired.target, retired.proof);
        }
    }

    fn retire(&mut self, retired: RetiredTarget) {
        if self.retired.len() >= self.retired_capacity && !self.retired.is_empty() {
            tracing::error!(
                target: "fasticamera::render::still",
                capacity = self.retired_capacity,
                "отложенных целей кодера больше ёмкости: самая старая оставлена в учтённую утечку"
            );
            // `Drop` цели оставляет её объекты в утечке и отпускает поверхность.
            drop(self.retired.remove(0));
        }
        self.retired.push(retired);
    }

    /// Цель, в которую ничего не отправлялось (отказ показа): неблокирующий разбор, иначе — отложить.
    fn dispose_unsubmitted(&mut self, target: EncoderInputTarget, held_window: Option<NativeSurface>) {
        match target.try_detach_unpresented() {
            Ok(DetachProgress::Detached(surface)) => release_encoder_surface(surface),
            Ok(DetachProgress::PresentsPending(target) | DetachProgress::DeviceWideWaitRequired(target)) => {
                // Картинка получена, а отправка сорвалась: семафор acquire висит, опросом не узнать.
                self.retire(RetiredTarget {
                    target: *target,
                    proof: TeardownProof::NeverSubmitted,
                    _held_window: held_window,
                });
            }
            Err(error) => {
                tracing::warn!(target: "fasticamera::render::still", %error, "цель кодера не разобрана после отказа");
            }
        }
    }
}

/// Блокирующий разбор цели (только остановка GPU снимков): GPU простаивает, ожидание ограничено реализацией.
fn detach_blocking(target: EncoderInputTarget, proof: TeardownProof) {
    let detached = match proof {
        TeardownProof::Copy(proof) => target.detach(&proof),
        TeardownProof::NeverSubmitted => target.detach_unpresented(),
    };
    match detached {
        Ok(surface) => release_encoder_surface(surface),
        Err(error) => {
            tracing::warn!(target: "fasticamera::render::still", %error, "цель кодера не разобрана при остановке");
        }
    }
}

/// Поверхность кодера как цель показа: на Android — окно как есть, на ПК-стенде — `Headless`, а окно держим сами.
#[cfg(target_os = "android")]
fn presentation_for_encoder(surface: NativeSurface) -> (PresentationSurface, Option<NativeSurface>) {
    (PresentationSurface::Android(surface), None)
}

#[cfg(not(target_os = "android"))]
fn presentation_for_encoder(surface: NativeSurface) -> (PresentationSurface, Option<NativeSurface>) {
    (PresentationSurface::Headless, Some(surface))
}

/// Отпускает поверхность кодера (на Android — ссылка на окно; на ПК-стенде `Headless` отпускать нечего).
/// Отдельная функция, чтобы намерение «здесь поверхность возвращается платформе» читалось на месте вызова.
fn release_encoder_surface(_surface: PresentationSurface) {}

#[cfg(all(test, feature = "fault-injection"))]
#[path = "present_tests.rs"]
mod tests;
