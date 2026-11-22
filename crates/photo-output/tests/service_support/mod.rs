//! Общие fake-сервисы и стенд для тестов службы записи снимков.
//!
//! Стенд поднимает НАСТОЯЩУЮ службу с её рабочим потоком; подделаны только платформа (хранилище,
//! кодеры, JPEG), часы, каталог камер, запись форматов и приёмники (фазы, цели рендера).

#![allow(dead_code, reason = "каждый тестовый бинарь использует свою часть стенда")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение — способ сообщить о провале"
)]

mod facts;
mod fakes;
mod sinks;

use std::sync::Arc;
use std::time::Duration;

#[allow(unused_imports, reason = "каждый тестовый бинарь берёт свою часть")]
pub use facts::{facts_for, raw_for, readback_for};
#[allow(unused_imports, reason = "каждый тестовый бинарь берёт свою часть")]
pub use fakes::{DraftState, EncoderEvent, FakeFormats, FakeHeif, FakeJpeg, FakeStore, FixedCameras, FixedClock};
use photo_output::service::{StillService, StillServiceConfig, StillServiceDeps, StillServiceHandle};
#[allow(unused_imports, reason = "каждый тестовый бинарь берёт свою часть")]
pub use sinks::{PhaseCollector, TargetCollector};
use still_contract::develop::{DevelopEvent, PrimaryTargetOffer};
use still_contract::phase::StillPhase;
use still_contract::plan::{PrimaryFormat, RawOutput, StillId};

/// Предел ожидания в тестах: достаточно для потока, мало для зависшего теста.
pub const WAIT: Duration = Duration::from_secs(5);

/// Поднятая служба и все подделки вокруг неё.
pub struct Rig {
    pub service: StillService,
    pub handle: StillServiceHandle,
    pub store: Arc<FakeStore>,
    pub heif: Arc<FakeHeif>,
    pub jpeg: Arc<FakeJpeg>,
    pub formats: Arc<FakeFormats>,
    pub phases: Arc<PhaseCollector>,
    pub targets: Arc<TargetCollector>,
    pub cameras: Arc<FixedCameras>,
}

/// Конфиг для тестов: короткие сроки, чтобы тесты на срок шли быстро.
pub fn fast_config() -> StillServiceConfig {
    StillServiceConfig {
        capture_timeout_ms: 2_000,
        develop_timeout_ms: 2_000,
        heif_finish_timeout_ms: 1_000,
        shutdown_timeout_ms: 3_000,
        ..StillServiceConfig::default()
    }
}

impl Rig {
    pub fn start() -> Self {
        Self::start_with(fast_config())
    }

    pub fn start_with(config: StillServiceConfig) -> Self {
        let store = Arc::new(FakeStore::default());
        let heif = Arc::new(FakeHeif::default());
        let jpeg = Arc::new(FakeJpeg::default());
        let formats = Arc::new(FakeFormats::default());
        let phases = Arc::new(PhaseCollector::default());
        let targets = Arc::new(TargetCollector::default());
        let cameras = Arc::new(FixedCameras::default());
        let deps = StillServiceDeps {
            store: store.clone(),
            heif: heif.clone(),
            jpeg: jpeg.clone(),
            formats: formats.clone(),
            clock: Arc::new(FixedClock),
            cameras: cameras.clone(),
            phases: phases.clone(),
            targets: targets.clone(),
        };
        let service = StillService::start(config, deps).expect("служба запускается");
        let handle = service.handle();
        Self {
            service,
            handle,
            store,
            heif,
            jpeg,
            formats,
            phases,
            targets,
            cameras,
        }
    }

    /// Допускает снимок и отдаёт билет службе, как это делает приложение.
    pub fn admit(&self, id: u64) {
        let ticket = self.handle.try_admit(StillId(id)).expect("допуск есть");
        self.handle.begin_still(ticket).expect("служба принимает билет");
    }

    /// Ждёт итоговую фазу снимка.
    pub fn wait_final(&self, id: u64) -> StillPhase {
        self.phases.wait_final(StillId(id), WAIT)
    }

    /// Рендер сообщает «кадр захвачен» (с сырыми данными, если план просил DNG) и получает ответ службы.
    pub fn secure(&self, id: u64, primary: PrimaryFormat, raw: RawOutput) -> PrimaryTargetOffer {
        let samples = (raw == RawOutput::Dng).then(raw_for);
        let event = DevelopEvent::Secured {
            facts: facts_for(id, primary, raw),
            raw: samples,
        };
        self.handle.submit_develop_event(event).expect("событие принято");
        self.targets.wait_offer(StillId(id), WAIT)
    }

    /// Отправляет событие рендера.
    pub fn send(&self, event: DevelopEvent) {
        self.handle.submit_develop_event(event).expect("событие принято");
    }
}

/// Ждёт выполнения условия (для эффектов, которые служба делает асинхронно).
pub fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + WAIT;
    while !condition() {
        assert!(std::time::Instant::now() < deadline, "не дождались: {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}
