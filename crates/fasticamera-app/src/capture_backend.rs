//! Набор «кто снимает» для экрана камеры: сервис снимка, адрес ответов, звук затвора и диагностика режима.
//!
//! Режим явный и виден в диагностике ([`CaptureMode`]):
//! * [`CaptureMode::Real`] — настоящая служба: кольцо кадров → проявление → файл в MediaStore;
//! * [`CaptureMode::Fake`] — фейк по замыслу (ПК, тесты, сценарии без камеры);
//! * [`CaptureMode::FakeFallback`] — фейк потому, что настоящее не поднялось (нет рендера, камеры, службы или
//!   паспорта телефона). Это НЕ молчаливая подмена: причина лежит в диагностике и пишется в лог при сборке.
//!
//! Экран и исполнитель эффектов о режиме не знают: им нужны только [`CaptureService`] и звук. Режим, журнал и
//! счётчики читает диагностика ([`CaptureMonitor`]) — одним интерфейсом для обоих режимов, чтобы сценарии не
//! зависели от того, что стоит за сервисом.

use std::sync::Arc;

use camera_screen::shutter::ShotRequest;
use photo_output::service::StillServiceStats;
use serde::Serialize;
use shell_contract::still_media::ShutterSoundService;

use crate::camera_screen_capture::{CaptureCompletionSlot, CaptureService, FakeCaptureCounters, FakeCaptureService};
use crate::camera_screen_config::CameraScreenConfig;
use crate::camera_screen_service::SilentShutterSound;
use crate::capture_switch::SwitchableCaptureService;
use crate::still_capture_service::CaptureCounters;
use crate::still_choice_log::ChoiceEntry;
use crate::still_event_forwarder::ForwarderCounters;
use crate::still_phase_log::{PhaseBatch, PhaseCursor};
use crate::still_screen_ledger::LedgerCounters;

const LOG_TARGET: &str = "fasticamera::still::backend";

/// Режим сервиса снимка.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum CaptureMode {
    /// Настоящая служба снимков.
    Real,
    /// Фейк по замыслу (ПК, тесты).
    Fake,
    /// Фейк, потому что настоящее не поднялось; `reason` — почему.
    FakeFallback { reason: String },
    /// Настоящее есть, но на время сценария `scenario` вместо него фейк (сценарий жмёт затвор, а запись в галерею
    /// ему не нужна). После сценария режим возвращается к `Real`.
    FakeForScenario { scenario: &'static str },
}

/// Диагностика фейка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FakeDiagnostics {
    pub counters: FakeCaptureCounters,
    /// Сколько раз экран просил звук затвора (звука нет: заглушка считает просьбы).
    pub shutter_sound_requests: u64,
}

/// Диагностика настоящей службы.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RealDiagnostics {
    /// Счётчики службы записи (допуск, итоги, срывы).
    pub service: StillServiceStats,
    /// Постановка запросов.
    pub capture: CaptureCounters,
    /// Пересылка событий рендера в службу.
    pub events: ForwarderCounters,
    /// Доставка фаз экрану.
    pub screen: LedgerCounters,
    /// Уборка осиротевших черновиков прошлого процесса.
    pub orphan_drafts: android_shell::still_orphan_sweep::OrphanSweepReport,
}

/// Всё, что сценарии узнают о съёмке: режим и счётчики того, что за ним стоит.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CaptureDiagnostics {
    #[serde(flatten)]
    pub mode: CaptureMode,
    /// Есть в режимах `fake` и `fake_fallback`.
    pub fake: Option<FakeDiagnostics>,
    /// Есть в режиме `real`.
    pub real: Option<RealDiagnostics>,
}

/// Чтение диагностики, журнала и счёта снимков, не зависящее от режима.
pub trait CaptureMonitor: Send + Sync {
    fn diagnostics(&self) -> CaptureDiagnostics;

    /// Последние принятые запросы по порядку (журнал ограничен, старые вытеснены).
    fn journal(&self) -> Vec<ShotRequest>;

    /// Итоги снимков (адреса файлов, время) после курсора. У фейка итогов службы нет — пустая порция.
    fn phases_after(&self, cursor: PhaseCursor) -> PhaseBatch;

    /// Курсор «сейчас»: фазы, записанные до него, читателю не нужны.
    fn phase_cursor_now(&self) -> PhaseCursor;

    /// Какие кадры выбрал рендер для снимков (хранимые, ограниченный журнал). У фейка рендера нет — пусто.
    fn frame_choices(&self) -> Vec<ChoiceEntry>;

    /// Сколько запросов снимка принято за процесс (не ограничено журналом; не зависит от режима).
    fn accepted_total(&self) -> u64;
}

/// Монитор фейка.
struct FakeMonitor {
    mode: CaptureMode,
    capture: Arc<FakeCaptureService>,
    sound: Arc<SilentShutterSound>,
}

impl CaptureMonitor for FakeMonitor {
    fn diagnostics(&self) -> CaptureDiagnostics {
        CaptureDiagnostics {
            mode: self.mode.clone(),
            fake: Some(FakeDiagnostics {
                counters: self.capture.counters(),
                shutter_sound_requests: self.sound.requests(),
            }),
            real: None,
        }
    }

    fn journal(&self) -> Vec<ShotRequest> {
        self.capture.journal()
    }

    fn accepted_total(&self) -> u64 {
        self.capture.counters().submitted
    }

    fn phases_after(&self, _cursor: PhaseCursor) -> PhaseBatch {
        PhaseBatch::default()
    }

    fn phase_cursor_now(&self) -> PhaseCursor {
        PhaseCursor::default()
    }

    fn frame_choices(&self) -> Vec<ChoiceEntry> {
        Vec::new()
    }
}

/// Фейк, подставленный на время сценария (см. [`CaptureMode::FakeForScenario`]).
struct ScenarioFake {
    scenario: &'static str,
    capture: Arc<FakeCaptureService>,
}

/// Что вышло из просьбы подставить фейк на время сценария.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FakeSubstitution {
    /// Настоящий сервис заменён фейком до конца сценария.
    Substituted,
    /// Подстановка не нужна: уже работает фейк (по замыслу или запасной), снимки и так не пишутся в галерею.
    NotNeeded,
}

/// Набор для экрана камеры.
pub struct CaptureBackend {
    /// То, что видит исполнитель эффектов: переключатель за одним `Arc` (см. [`SwitchableCaptureService`]).
    service: Arc<SwitchableCaptureService>,
    completion_slot: Arc<CaptureCompletionSlot>,
    shutter_sound: Arc<dyn ShutterSoundService>,
    monitor: Arc<dyn CaptureMonitor>,
    /// Фейк на время сценария; `None` — снимает тот, кто собран при сборке набора.
    scenario_fake: Option<ScenarioFake>,
}

/// Части настоящего набора (собирает `still_runtime`).
pub struct RealBackendParts {
    pub service: Arc<dyn CaptureService>,
    pub completion_slot: Arc<CaptureCompletionSlot>,
    pub shutter_sound: Arc<dyn ShutterSoundService>,
    pub monitor: Arc<dyn CaptureMonitor>,
}

impl CaptureBackend {
    /// Фейк по замыслу (сроки и журнал — из конфига экрана).
    pub fn fake(config: &CameraScreenConfig) -> Self {
        Self::fake_in_mode(config, CaptureMode::Fake)
    }

    /// Фейк вместо настоящего, которое не поднялось: причина в диагностике и логе.
    pub fn fake_fallback(config: &CameraScreenConfig, reason: impl Into<String>) -> Self {
        let reason = reason.into();
        tracing::error!(target: LOG_TARGET, %reason, "настоящий сервис снимка не поднялся: работает ФЕЙК (снимки не пишутся в галерею)");
        Self::fake_in_mode(config, CaptureMode::FakeFallback { reason })
    }

    fn fake_in_mode(config: &CameraScreenConfig, mode: CaptureMode) -> Self {
        let completion_slot = Arc::new(CaptureCompletionSlot::default());
        let capture = Arc::new(FakeCaptureService::new(
            config.fake_capture_timing(),
            config.fake_capture_journal_capacity,
            Arc::clone(&completion_slot),
        ));
        let sound = Arc::new(SilentShutterSound::default());
        Self {
            service: Arc::new(SwitchableCaptureService::new(
                Arc::clone(&capture) as Arc<dyn CaptureService>
            )),
            completion_slot,
            shutter_sound: Arc::clone(&sound) as Arc<dyn ShutterSoundService>,
            monitor: Arc::new(FakeMonitor { mode, capture, sound }),
            scenario_fake: None,
        }
    }

    /// Настоящий набор из готовых частей.
    pub fn real(parts: RealBackendParts) -> Self {
        Self {
            service: Arc::new(SwitchableCaptureService::new(parts.service)),
            completion_slot: parts.completion_slot,
            shutter_sound: parts.shutter_sound,
            monitor: parts.monitor,
            scenario_fake: None,
        }
    }

    pub(crate) fn service(&self) -> Arc<dyn CaptureService> {
        Arc::clone(&self.service) as Arc<dyn CaptureService>
    }

    /// Подставляет фейк на время сценария `scenario`, если сейчас снимает НАСТОЯЩИЙ сервис.
    ///
    /// Фейк отвечает через ТУ ЖЕ ячейку адреса экрана, что и настоящий сервис: ответы дойдут до текущего экрана
    /// (новая ячейка никогда не была бы подключена, и затвор навсегда остался бы в «снимается»). Повторный вызов
    /// без возврата — `NotNeeded`: подстановка уже идёт.
    pub(crate) fn substitute_fake_for_scenario(
        &mut self,
        scenario: &'static str,
        config: &CameraScreenConfig,
    ) -> FakeSubstitution {
        if self.scenario_fake.is_some() || self.monitor.diagnostics().mode != CaptureMode::Real {
            return FakeSubstitution::NotNeeded;
        }
        let capture = Arc::new(FakeCaptureService::new(
            config.fake_capture_timing(),
            config.fake_capture_journal_capacity,
            Arc::clone(&self.completion_slot),
        ));
        self.service.substitute(Arc::clone(&capture) as Arc<dyn CaptureService>);
        tracing::info!(target: LOG_TARGET, scenario, "на время сценария снимки идут в фейк: файлы в галерею не пишутся");
        self.scenario_fake = Some(ScenarioFake { scenario, capture });
        FakeSubstitution::Substituted
    }

    /// Возвращает настоящий сервис. `true` — подстановка была. Звать при ЛЮБОМ конце сценария.
    pub(crate) fn restore_after_scenario(&mut self) -> bool {
        let restored = self.service.restore();
        if let Some(fake) = self.scenario_fake.take() {
            tracing::info!(target: LOG_TARGET, scenario = fake.scenario, "сценарий кончился: снимки снова настоящие");
        }
        restored
    }

    pub(crate) fn completion_slot(&self) -> &Arc<CaptureCompletionSlot> {
        &self.completion_slot
    }

    pub(crate) fn shutter_sound(&self) -> Arc<dyn ShutterSoundService> {
        Arc::clone(&self.shutter_sound)
    }

    pub fn diagnostics(&self) -> CaptureDiagnostics {
        let mut diagnostics = self.monitor.diagnostics();
        if let Some(fake) = &self.scenario_fake {
            diagnostics.mode = CaptureMode::FakeForScenario {
                scenario: fake.scenario,
            };
            diagnostics.fake = Some(FakeDiagnostics {
                counters: fake.capture.counters(),
                // Звук затвора остаётся настоящим и здесь не считается.
                shutter_sound_requests: 0,
            });
        }
        diagnostics
    }

    /// Журнал запросов того, кто СЕЙЧАС отвечает: у подставного фейка свой, настоящий журнал не мешается.
    pub fn journal(&self) -> Vec<ShotRequest> {
        match &self.scenario_fake {
            Some(fake) => fake.capture.journal(),
            None => self.monitor.journal(),
        }
    }

    pub fn accepted_total(&self) -> u64 {
        match &self.scenario_fake {
            Some(fake) => fake.capture.counters().submitted,
            None => self.monitor.accepted_total(),
        }
    }

    /// Итоги настоящих снимков после курсора (адреса файлов, время фаз).
    pub fn phases_after(&self, cursor: PhaseCursor) -> PhaseBatch {
        self.monitor.phases_after(cursor)
    }

    /// Курсор «сейчас» журнала итогов.
    pub fn phase_cursor_now(&self) -> PhaseCursor {
        self.monitor.phase_cursor_now()
    }

    /// Выборы кадра настоящих снимков (кого рендер взял для каждого `StillId`).
    pub fn frame_choices(&self) -> Vec<ChoiceEntry> {
        self.monitor.frame_choices()
    }
}

/// Как маршрутизатор получает набор: фейк по умолчанию или готовый (его собирает composition root).
pub enum CaptureWiring {
    /// Фейк по замыслу, собирается из конфига экрана.
    Fake,
    /// Готовый набор (настоящий или фейк-запасной с причиной).
    Backend(CaptureBackend),
}

impl CaptureWiring {
    pub(crate) fn into_backend(self, config: &CameraScreenConfig) -> CaptureBackend {
        match self {
            Self::Fake => CaptureBackend::fake(config),
            Self::Backend(backend) => backend,
        }
    }
}

#[cfg(test)]
#[path = "capture_backend_tests.rs"]
mod tests;
