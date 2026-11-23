//! Движок снимков: очередь, протокол событий, ленты от видоискателя. Шаги по кадрам — в `engine_steps`.
//!
//! # Кто чем владеет
//! * движок — очередь снимков ([`StillEntry`]), журнал показов, историю параметров кадров, счётчики;
//! * [`StillGpu`] — железо (создаётся лениво через [`GpuMaker`]; живёт, пока есть surface ИЛИ недоделанные снимки:
//!   без surface захваченные снимки доводятся до файла, решение 150, см. `engine_surface`);
//! * кольцо снимков — источник кадров (`SnapshotRingFrames`, неразрушающий `pick`).
//!
//! # Инварианты
//! 1. Каждый принятый `Begin` кончается РОВНО одним итоговым событием: итог доставляет только
//!    [`StillEngine::finish`], а он забирает запись по значению (см. `entry`).
//! 2. Поток рендера не ждёт: GPU опрашивается, а не ждётся (исключение — [`StillEngine::abandon_all`], где ожидание
//!    ограничено реализацией GPU). Исход GPU неизвестен (сбой опроса, срок) — GPU объявляется потерянным и его
//!    объекты остаются в учтённой утечке (`gpu_loss`), а не уничтожаются.
//! 3. Один снимок копируется и один проявляется за раз (память); полос на кадр не больше `slices_per_frame`.
//! 4. Лизинг кадра кольца у движка живёт только внутри `start_capture`: дальше им владеет GPU до fence копии.
//! 5. Пропажа surface не бросает снимки, у которых уже есть объекты GPU или своя копия RAW: они доводятся шагами
//!    по таймеру потока рендера. Бросаются только те, кому нужен экран (выбор кадра, изъятие из кольца), — честным
//!    `NotSecured(RendererStopped)`, без ожидания GPU (`engine_surface`).

use std::sync::Arc;
use std::time::Instant;

use camera_contract::frame_passport::StreamPassport;
use isp_model::lens_shading::LensShadingControl;
use raw_frame_stream::SnapshotRingFrames;
use still_contract::develop::{DevelopFailure, PrimaryTargetOffer};
use still_contract::frame_choice::NoFrameReason;

use super::diagnostics::StillDiagnostics;
use super::engine_surface::SurfaceLink;
use super::entry::{EstimateBasis, Stage, StillEntry, StillOutcome};
use super::feed::StillFeed;
use super::flash_facts_store::FlashFactsStore;
use super::gpu::StillGpu;
use super::params_history::FrameParamsHistory;
use super::present_log::PresentLog;
use crate::backend::PastPresentation;
use crate::config::{StillDevelopConfig, ViewfinderToneConfig};
use crate::still::{StillBegin, StillCommand, StillCommandOutcome};
use crate::vulkan_backend::camera::exposure::ResidualGainLimits;

/// Часы движка: отдельный шов, чтобы сроки проверялись тестами без ожидания.
pub trait StillClock: Send {
    fn now(&self) -> Instant;
}

/// Системные часы.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemStillClock;

impl StillClock for SystemStillClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Создаёт GPU снимков под поток кадров (полезная область потока определяет размеры буферов).
pub type GpuMaker<G> = Box<dyn FnMut(&Arc<StreamPassport>) -> Result<G, String> + Send>;

/// Движок снимков одного бэкенда.
pub struct StillEngine<G: StillGpu> {
    pub(super) config: StillDevelopConfig,
    /// Ручки тона видоискателя: снимок проявляется тем же тоном (`viewfinder.tone`).
    pub(super) tone: ViewfinderToneConfig,
    /// Коррекция виньетирования: снимок использует ту же секцию, что видоискатель (`viewfinder.lens_shading`).
    pub(super) lens_shading: LensShadingControl,
    pub(super) ring: SnapshotRingFrames,
    pub(super) clock: Box<dyn StillClock>,
    pub(super) maker: GpuMaker<G>,
    pub(super) gpu: Option<G>,
    /// Очередь по порядку прихода.
    pub(super) queue: Vec<StillEntry<G>>,
    pub(super) present_log: PresentLog,
    pub(super) params_history: FrameParamsHistory,
    /// Планы, белая светодиода и свет сцены для кадра со вспышкой (14d).
    pub(super) flash_facts: FlashFactsStore,
    /// Шаг и предел остаточной подтяжки видоискателя: кадр со вспышкой округляет подтяжку так же.
    pub(super) residual_gain: ResidualGainLimits,
    /// Поток последнего обработанного кадра: под него заранее создаётся GPU.
    pub(super) current_stream: Option<Arc<StreamPassport>>,
    /// Поток, для которого создание GPU заранее уже не удалось (повторять до смены потока незачем).
    pub(super) prewarm_failed_for: Option<Arc<StreamPassport>>,
    /// Есть ли у видоискателя surface: без неё опустевшая очередь освобождает GPU снимков (`engine_surface`).
    pub(super) surface: SurfaceLink,
    pub(super) counters: StillDiagnostics,
}

impl<G: StillGpu> StillEngine<G> {
    /// Тот же движок с управлением виньетированием из конфига (`viewfinder.lens_shading`): снимок проявляется
    /// с той же коррекцией, что видоискатель. Вызывается при создании, до первого снимка.
    #[must_use]
    pub fn with_lens_shading(mut self, lens_shading: LensShadingControl) -> Self {
        self.lens_shading = lens_shading;
        self
    }

    /// Движок поверх кольца снимков. GPU создаётся лениво через `maker`.
    pub fn new(
        config: StillDevelopConfig,
        tone: ViewfinderToneConfig,
        residual_gain: ResidualGainLimits,
        ring: SnapshotRingFrames,
        maker: GpuMaker<G>,
        clock: Box<dyn StillClock>,
    ) -> Self {
        let present_capacity = usize::try_from(config.present_log_capacity).unwrap_or(usize::MAX);
        let params_capacity = usize::try_from(config.frame_params_capacity).unwrap_or(usize::MAX);
        Self {
            config,
            tone,
            lens_shading: LensShadingControl::default(),
            ring,
            clock,
            maker,
            gpu: None,
            queue: Vec::new(),
            present_log: PresentLog::new(present_capacity),
            params_history: FrameParamsHistory::new(params_capacity),
            flash_facts: FlashFactsStore::default(),
            residual_gain,
            current_stream: None,
            prewarm_failed_for: None,
            // До первой ленты видоискателя surface у движка не было: так честнее, чем «присоединена».
            surface: SurfaceLink::Detached,
            counters: StillDiagnostics::default(),
        }
    }

    // ---- команды ----

    /// Принимает команду потока рендера. `Unsupported` — только для цели, которой некуда лечь (снимка нет или он
    /// не ждёт цель): команда возвращается потоку, он её уничтожает (поверхность кодера отпускается).
    pub fn apply_command(&mut self, command: StillCommand) -> StillCommandOutcome {
        match command {
            StillCommand::Begin(begin) => {
                self.begin(begin);
                StillCommandOutcome::Accepted
            }
            StillCommand::Target(offer) => {
                let outcome = self.accept_target(offer);
                // Отказ цели (`Unavailable`) мог дать итог последнему снимку вне шага: без surface поток рендера
                // больше не проснётся ради пустой очереди, поэтому GPU отпускается здесь же (`engine_surface`).
                self.release_gpu_if_drained_without_surface();
                outcome
            }
            StillCommand::FlashFailed { id, failure } => {
                self.apply_flash_failed(id, failure);
                // Снимок мог остаться последним в очереди без surface: GPU отпускается здесь же.
                self.release_gpu_if_drained_without_surface();
                StillCommandOutcome::Accepted
            }
        }
    }

    /// Отказ `Begin`, пришедшему без присоединённой surface: снимок берёт кадр, бывший НА ЭКРАНЕ в момент нажатия,
    /// а экрана нет — выбирать нечего. Итог тот же, что у не захваченного снимка при отсоединении:
    /// `NotSecured(RendererStopped)`, ровно один.
    pub fn refuse_begin_without_surface(&mut self, begin: StillBegin) {
        self.counters.begins += 1;
        self.counters.refusals.begin_without_surface += 1;
        let StillBegin { plan, events } = begin;
        // Стадия фиктивная: запись нужна лишь как носитель инварианта «ровно один итог».
        let entry = StillEntry::new(
            plan,
            events,
            Stage::Choosing {
                deadline: self.clock.now(),
                estimate: EstimateBasis::NoLiveFrame,
            },
        );
        self.finish(entry, StillOutcome::NotSecured(NoFrameReason::RendererStopped));
    }

    fn begin(&mut self, begin: StillBegin) {
        self.counters.begins += 1;
        let StillBegin { plan, events } = begin;
        let estimate = match self.present_log.latest_shown() {
            Some(super::feed::ShownContent::Live(sensor_timestamp)) => EstimateBasis::Live(sensor_timestamp),
            Some(super::feed::ShownContent::NotLive) | None => EstimateBasis::NoLiveFrame,
        };
        // Снимок со вспышкой ждёт кадр запроса, а не доказательства показа (`flash_frame`).
        let stage = match Self::flash_shot_of(plan.selection) {
            Some(shot) => {
                self.counters.flash.begins += 1;
                Stage::AwaitingFlashFrame {
                    shot,
                    deadline: self.flash_wait_deadline(),
                }
            }
            None => {
                let deadline =
                    self.clock.now() + std::time::Duration::from_millis(u64::from(self.config.presentation_wait_ms));
                Stage::Choosing { deadline, estimate }
            }
        };
        let entry = StillEntry::new(plan, events, stage);
        if self.ring.capacity() == 0 {
            self.counters.picks.no_frame_ring_disabled += 1;
            return self.finish(entry, StillOutcome::NotSecured(NoFrameReason::RingDisabled));
        }
        let queue_capacity = usize::try_from(self.config.queue_capacity).unwrap_or(usize::MAX);
        if self.queue.len() >= queue_capacity {
            self.counters.refusals.queue_full += 1;
            return self.finish(entry, StillOutcome::NotSecured(NoFrameReason::RendererBusy));
        }
        self.queue.push(entry);
        self.counters.queue_high_water = self.counters.queue_high_water.max(self.queue.len());
    }

    fn accept_target(&mut self, offer: PrimaryTargetOffer) -> StillCommandOutcome {
        let id = offer.id();
        let position = self
            .queue
            .iter()
            .position(|entry| entry.plan.id == id && matches!(entry.stage, Stage::AwaitingTarget { .. }));
        let Some(index) = position else {
            self.counters.targets_unexpected += 1;
            return StillCommandOutcome::Unsupported(StillCommand::Target(offer));
        };
        let mut entry = self.queue.remove(index);
        match offer {
            PrimaryTargetOffer::Ready { target, .. } => {
                if let Stage::AwaitingTarget { inputs, .. } = entry.take_stage() {
                    entry.stage = Stage::AwaitingDevelop { target, inputs };
                }
                self.queue.insert(index, entry);
            }
            // Контракт `develop.rs`: цели нет — рендер отпускает снимок итогом `TargetUnavailable`.
            PrimaryTargetOffer::Unavailable { .. } => {
                self.finish(entry, StillOutcome::Failed(DevelopFailure::TargetUnavailable));
            }
        }
        StillCommandOutcome::Accepted
    }

    // ---- ленты от видоискателя и драйвера ----

    /// Принимает ленту видоискателя за кадр (показы и параметры обработанных кадров).
    pub fn ingest_feed(&mut self, feed: StillFeed) {
        // Лента приходит только от присоединённой сессии видоискателя: surface снова есть.
        self.surface = SurfaceLink::Attached;
        if feed.presents_restarted {
            self.present_log.restart();
        }
        for record in feed.presents {
            self.present_log.record_shown(record);
        }
        self.flash_facts
            .record(feed.flash_plans, feed.torch_whites, feed.ambient_levels);
        let got_params = !feed.params.is_empty();
        for (sensor_timestamp, params) in feed.params {
            self.current_stream = Some(Arc::clone(&params.stream));
            self.params_history.record(sensor_timestamp, params);
        }
        if got_params {
            self.prewarm_gpu();
        }
    }

    /// Принимает записи показа драйвера (`present_id` → момент на экране).
    pub fn ingest_presentations(&mut self, presentations: &[PastPresentation]) {
        for presentation in presentations {
            self.present_log
                .record_timing(presentation.present_id, presentation.actual_present);
        }
    }

    // ---- итоги ----

    /// ЕДИНСТВЕННОЕ место, где снимок получает итог: считает его и доставляет, забирая запись.
    pub(super) fn finish(&mut self, entry: StillEntry<G>, outcome: StillOutcome) {
        let counters = &mut self.counters.outcomes;
        match &outcome {
            StillOutcome::NotSecured(_) => counters.not_secured += 1,
            StillOutcome::Rendered => counters.rendered += 1,
            StillOutcome::Readback(_) => counters.readback += 1,
            StillOutcome::Failed(failure) => match failure {
                DevelopFailure::ParamsUnavailable(_) => counters.failed_params_unavailable += 1,
                DevelopFailure::Gpu(_) => counters.failed_gpu += 1,
                DevelopFailure::TargetUnavailable => counters.failed_target_unavailable += 1,
                DevelopFailure::TargetTimedOut => counters.failed_target_timed_out += 1,
                DevelopFailure::Abandoned => counters.failed_abandoned += 1,
            },
        }
        entry.deliver_final(outcome);
    }

    // ---- остановка ----

    /// Бросает все незаконченные снимки: поток рендера останавливается или неисправен (бэкенд уничтожается,
    /// `DisplayBackend::abandon_stills`). Пропажа surface сюда НЕ ведёт (`engine_surface::on_detach`).
    ///
    /// Порядок: сначала GPU (ограниченное ожидание всей отправленной работы), потом итоги записям. Если GPU не
    /// освободился за срок, объекты GPU снимков намеренно утекают (соглашение crate): память, в которую ещё может
    /// писать GPU, не уничтожается.
    pub fn abandon_all(&mut self) {
        let gpu_idle = match self.gpu.as_mut() {
            Some(gpu) => gpu.abandon().gpu_idle,
            None => true,
        };
        if !gpu_idle {
            self.counters.gpu_leaked_on_abandon += 1;
            tracing::error!(
                target: "fasticamera::render::still",
                "GPU снимков не освободился: объекты проявления намеренно оставлены в утечку"
            );
        }
        let entries: Vec<StillEntry<G>> = self.queue.drain(..).collect();
        for mut entry in entries {
            if entry.is_secured() {
                self.counters.abandoned_after_capture += 1;
            } else {
                self.counters.abandoned_before_capture += 1;
            }
            if !gpu_idle {
                entry.leak_gpu_objects();
            }
            let outcome = entry.abandonment_outcome();
            self.finish(entry, outcome);
        }
        if let Some(gpu) = self.gpu.take()
            && !gpu_idle
        {
            std::mem::forget(gpu);
        }
    }

    // ---- диагностика ----

    /// Снимок счётчиков.
    pub fn diagnostics(&self) -> StillDiagnostics {
        StillDiagnostics {
            queue_len: self.queue.len(),
            ..self.counters.clone()
        }
    }

    /// Сколько снимков сейчас в очереди (тесты).
    #[cfg(test)]
    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }

    /// Создан ли GPU снимков (тесты).
    #[cfg(test)]
    pub fn has_gpu(&self) -> bool {
        self.gpu.is_some()
    }
}

impl<G: StillGpu> Drop for StillEngine<G> {
    /// Бэкенд уничтожен (остановка потока, паника): каждый снимок получает итог, GPU освобождается по тем же
    /// правилам, что при отсоединении.
    fn drop(&mut self) {
        self.abandon_all();
    }
}
