//! Запись очереди снимков: один принятый `Begin` от нажатия до единственного итога.
//!
//! # Инвариант «ровно один итог»
//! Итог доставляется ТОЛЬКО методом [`StillEntry::deliver_final`], который забирает запись по значению и
//! забирает у неё приёмник событий. Повторно доставить итог нельзя по построению: записи уже нет. Если запись
//! уничтожена, не получив итога (паника, ранний выход), `Drop` доставляет запасной итог и пишет ошибку в журнал.

use std::sync::Arc;
use std::time::Instant;

use camera_contract::capture_result::SensorTimestamp;
use camera_contract::flash::FlashShotId;
use raw_frame_stream::RingPickRequest;
use still_contract::develop::{DevelopEvent, DevelopFailure, PrimaryTarget};
use still_contract::frame_choice::NoFrameReason;
use still_contract::plan::StillPlan;
use still_contract::shot::{Rgba8Image, StillFrameFacts};

use super::choice::ShownAt;
use super::gpu::StillGpu;
use crate::still::StillEventSink;
use crate::vulkan_backend::camera::isp_params::IspParamsCache;

/// Что взять за оценку, если момент нажатия доказать не удастся: кадр, стоявший на экране к приходу команды.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstimateBasis {
    /// Последний живой кадр на экране.
    Live(SensorTimestamp),
    /// Живого кадра на экране не было (нет показов, удержание, чёрный).
    NoLiveFrame,
}

/// Какой кадр забрать из кольца, когда освободится копирование.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PickPlan {
    pub request: RingPickRequest,
    /// Когда кадр впервые был на экране, если это доказано.
    pub shown_at: ShownAt,
}

/// Всё, что нужно проявлению снимка после захвата.
pub struct DevelopInputs<G: StillGpu> {
    /// Копия RAW снимка (живёт до конца проявления: GPU читает её полосами).
    pub raw: G::Raw,
    pub facts: Box<StillFrameFacts>,
    /// Параметры ISP снимка (кэш с посчитанными параметрами этого кадра).
    pub params: Box<IspParamsCache>,
}

/// Где снимок в очереди.
pub enum Stage<G: StillGpu> {
    /// Ждёт доказательства, какой кадр был на экране в момент нажатия.
    Choosing { deadline: Instant, estimate: EstimateBasis },
    /// Снимок со вспышкой (14d): ждёт, пока кадр разового запроса `shot` дойдёт до кольца. Кольцо и срок —
    /// вся зависимость: показы видоискателя не нужны, но без surface кольца и видоискателя тоже нет.
    AwaitingFlashFrame { shot: FlashShotId, deadline: Instant },
    /// Кадр определён, ждёт свободного копирования.
    AwaitingCapture(PickPlan),
    /// Копия RAW идёт на GPU (кадр уже изъят из кольца).
    Capturing {
        facts: Box<StillFrameFacts>,
        params: Box<IspParamsCache>,
        /// Срок копии (`capture_timeout_ms`): зависший fence не должен держать лизинг камеры вечно.
        deadline: Instant,
    },
    /// `Secured` отправлен; ждём цель кодера от записи.
    AwaitingTarget {
        deadline: Instant,
        inputs: DevelopInputs<G>,
    },
    /// Цель получена, ждёт свободного проявителя.
    AwaitingDevelop {
        target: PrimaryTarget,
        inputs: DevelopInputs<G>,
    },
    /// Проявляется (полосами между кадрами видоискателя).
    Developing {
        target: PrimaryTarget,
        started: Instant,
        /// Копия RAW живёт до конца проявления: GPU читает её полосами.
        _inputs: DevelopInputs<G>,
    },
    /// Проявлено, ждёт свободного показа в поверхность кодера.
    Developed {
        surface: shell_contract::surface::NativeSurface,
        image: G::Image,
    },
    /// Картинка показывается в поверхность кодера.
    Presenting {
        deadline: Instant,
        /// Картинка живёт до конца показа: GPU читает её копией.
        _image: G::Image,
    },
    /// Временная пустота на время перехода между стадиями (`take_stage`): в очереди не живёт.
    Transit,
}

impl<G: StillGpu> Stage<G> {
    /// Держит ли стадия объекты GPU снимков или идущую на GPU работу (копия RAW, проявление, показ). Такие стадии
    /// нельзя просто выбросить, когда исход GPU неизвестен: их объекты остаются в учтённой утечке.
    pub fn holds_gpu_objects(&self) -> bool {
        match self {
            Self::Capturing { .. }
            | Self::AwaitingTarget { .. }
            | Self::AwaitingDevelop { .. }
            | Self::Developing { .. }
            | Self::Developed { .. }
            | Self::Presenting { .. } => true,
            Self::Choosing { .. } | Self::AwaitingFlashFrame { .. } | Self::AwaitingCapture(_) | Self::Transit => false,
        }
    }
}

/// Итог снимка (то, что станет единственным итоговым событием).
#[derive(Debug)]
pub enum StillOutcome {
    /// Кадра нет, захвата не было.
    NotSecured(NoFrameReason),
    /// Проявление сорвалось после захвата.
    Failed(DevelopFailure),
    /// Картинка показана в поверхность кодера.
    Rendered,
    /// Картинка прочитана в память.
    Readback(Rgba8Image),
}

/// Запись очереди.
pub struct StillEntry<G: StillGpu> {
    pub plan: StillPlan,
    pub stage: Stage<G>,
    /// `Some` до доставки итога.
    events: Option<Arc<dyn StillEventSink>>,
    /// `Secured` уже отправлен (определяет запасной итог).
    secured: bool,
    /// Кадр со вспышкой этой записи уже найден в кольце и посчитан (повторные находки не считаются).
    flash_frame_found: bool,
}

impl<G: StillGpu> StillEntry<G> {
    pub fn new(plan: StillPlan, events: Arc<dyn StillEventSink>, stage: Stage<G>) -> Self {
        Self {
            plan,
            stage,
            events: Some(events),
            secured: false,
            flash_frame_found: false,
        }
    }

    /// Отмечает находку кадра со вспышкой; `true` — первая находка этой записи (её и надо посчитать).
    pub fn note_flash_frame_found(&mut self) -> bool {
        !std::mem::replace(&mut self.flash_frame_found, true)
    }

    /// Отправлено ли `Secured`.
    pub fn is_secured(&self) -> bool {
        self.secured
    }

    /// Доставляет `Secured` (не итоговое событие: запись остаётся в очереди).
    pub fn deliver_secured(&mut self, event: DevelopEvent) {
        debug_assert!(
            !event.is_final(),
            "итоговое событие доставляется только через deliver_final"
        );
        if let Some(events) = &self.events {
            events.deliver(event);
        }
        self.secured = true;
    }

    /// Доставляет итог и забирает запись: больше событий у снимка не будет.
    pub fn deliver_final(mut self, outcome: StillOutcome) {
        let id = self.plan.id;
        let event = match outcome {
            StillOutcome::NotSecured(reason) => DevelopEvent::NotSecured { id, reason },
            StillOutcome::Failed(failure) => DevelopEvent::Failed { id, failure },
            StillOutcome::Rendered => DevelopEvent::PrimaryRendered { id },
            StillOutcome::Readback(image) => DevelopEvent::PrimaryReadback { id, image },
        };
        if let Some(events) = self.events.take() {
            events.deliver(event);
        }
    }

    /// Намеренно оставляет объекты GPU записи в утечку (GPU не освободился: уничтожать память, которую ещё может
    /// писать GPU, нельзя; соглашение crate «лучше утечка, чем use-after-free»).
    pub fn leak_gpu_objects(&mut self) {
        std::mem::forget(self.take_stage());
    }

    /// Забирает стадию, оставляя [`Stage::Transit`]: так данные стадии переезжают в следующую по значению
    /// (у записи есть `Drop`, поля из неё не вынуть). Вызывающий обязан сразу записать новую стадию.
    pub fn take_stage(&mut self) -> Stage<G> {
        std::mem::replace(&mut self.stage, Stage::Transit)
    }

    /// Итог брошенного снимка: до захвата — «кадра нет», после — «проявление брошено».
    pub fn abandonment_outcome(&self) -> StillOutcome {
        if self.secured {
            StillOutcome::Failed(DevelopFailure::Abandoned)
        } else {
            StillOutcome::NotSecured(NoFrameReason::RendererStopped)
        }
    }
}

impl<G: StillGpu> Drop for StillEntry<G> {
    /// Запасной итог: запись уничтожена без итога (паника в логике или ранний выход). Молча терять снимок нельзя —
    /// запись ждала бы вечно.
    fn drop(&mut self) {
        let Some(events) = self.events.take() else { return };
        tracing::error!(
            target: "fasticamera::render::still",
            id = self.plan.id.0,
            secured = self.secured,
            "снимок уничтожен без итога: доставлен запасной итог"
        );
        let id = self.plan.id;
        events.deliver(if self.secured {
            DevelopEvent::Failed {
                id,
                failure: DevelopFailure::Abandoned,
            }
        } else {
            DevelopEvent::NotSecured {
                id,
                reason: NoFrameReason::RendererStopped,
            }
        });
    }
}
