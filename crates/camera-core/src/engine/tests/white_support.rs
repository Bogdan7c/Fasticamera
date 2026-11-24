//! Общие помощники тестов белой: стенд с ящиками и памятью, ответы источника, чтение указаний и событий.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use camera_contract::fixtures::camera_id;
use camera_contract::three_a::{DirectiveSlot, RequestOrigin};
use camera_contract::white::{
    HeldWhite, SceneIlluminant, WhiteAnswer, WhiteAnswerChannel, WhiteConfidence, WhiteDirective, WhiteRequestId,
    WhiteSourceOutcome,
};

use super::three_a_support::result_meta;
use super::*;
use crate::white::{WhiteMemory, WhiteMemoryLoad, WhiteMemoryRecord, WhiteRequestResult, WhiteStatus, WhiteWiring};

/// Фальшивая память: складывает записи туда, где их видит тест.
struct RecordingMemory {
    saved: Arc<Mutex<Vec<WhiteMemoryRecord>>>,
}

impl WhiteMemory for RecordingMemory {
    fn save(&mut self, record: WhiteMemoryRecord) {
        self.saved.lock().unwrap_or_else(PoisonError::into_inner).push(record);
    }
}

/// То, что тест держит снаружи движка: читатель указаний (рендер), писатель ответов (источник), память.
pub(super) struct WhiteOutside {
    pub(super) directives: DirectiveSlot<WhiteDirective>,
    pub(super) answers: WhiteAnswerChannel,
    saved: Arc<Mutex<Vec<WhiteMemoryRecord>>>,
}

impl WhiteOutside {
    /// Последнее опубликованное указание и номер его поколения.
    pub(super) fn latest(&self) -> (u64, WhiteDirective) {
        let (generation, directive) = self
            .directives
            .latest()
            .expect("указание опубликовано при создании движка");
        (generation.0, directive)
    }

    pub(super) fn directive(&self) -> WhiteDirective {
        self.latest().1
    }

    pub(super) fn generation(&self) -> u64 {
        self.latest().0
    }

    /// Всё, что движок передал памяти, в порядке передачи.
    pub(super) fn saved(&self) -> Vec<WhiteMemoryRecord> {
        self.saved.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// Подключения белой: свои ящики, фальшивая память, заданная запомненная запись.
pub(super) fn white_wiring(remembered: WhiteMemoryLoad) -> (WhiteWiring, WhiteOutside) {
    let saved = Arc::new(Mutex::new(Vec::new()));
    let wiring = WhiteWiring {
        remembered,
        memory: Box::new(RecordingMemory {
            saved: Arc::clone(&saved),
        }),
        ..WhiteWiring::default()
    };
    let outside = WhiteOutside {
        directives: wiring.directives.clone(),
        answers: wiring.answers.clone(),
        saved,
    };
    (wiring, outside)
}

/// Стенд с белой; каталог прочитан, события старта в журнале.
pub(super) fn white_rig_with(
    config: CameraEngineConfig,
    presence: AppPresence,
    remembered: WhiteMemoryLoad,
) -> (Rig, WhiteOutside) {
    let (white, outside) = white_wiring(remembered);
    let wiring = ThreeAWiring::vendor(&config.three_a).with_white(white);
    let mut rig = Rig::over_fake_with_wiring(config, presence, FakeBackendControl::new(), wiring);
    rig.engine.load_catalog(rig.now);
    rig.pump();
    rig.fake.take_calls();
    (rig, outside)
}

/// Приложение стартует в фоне, как в проде; памяти нет.
pub(super) fn white_rig() -> (Rig, WhiteOutside) {
    white_rig_with(
        CameraEngineConfig::default(),
        AppPresence::Background,
        WhiteMemoryLoad::NothingRemembered,
    )
}

/// Стенд, в котором приложение уже вернулось на передний план и идёт поток; заявка открытия выдана (номер 1).
pub(super) fn white_rig_streaming() -> (Rig, WhiteOutside) {
    let (mut rig, outside) = white_rig();
    assert_eq!(rig.send(CameraCommand::EnterForeground), CommandOutcome::Accepted);
    rig.start_streaming_on_wide();
    (rig, outside)
}

pub(super) fn setting(kelvin: f64, duv: f64) -> WhiteBalanceSetting {
    WhiteBalanceSetting {
        temperature: ColorTemperature::from_kelvin(kelvin).expect("температура"),
        tint: Tint::from_duv(duv).expect("оттенок"),
    }
}

/// Ответ источника на заявку `request`.
pub(super) fn answer(request: u64, outcome: WhiteSourceOutcome) -> WhiteAnswer {
    WhiteAnswer {
        request: WhiteRequestId(request),
        camera: camera_contract::auto_exposure::CameraTag::of(&camera_id("2")),
        measured_frame: None,
        outcome,
    }
}

/// «Готово: вот такой свет».
pub(super) fn ready(request: u64, white: WhiteBalanceSetting) -> WhiteAnswer {
    answer(
        request,
        WhiteSourceOutcome::Ready {
            illuminant: SceneIlluminant::Uniform(white),
            confidence: WhiteConfidence::FULL,
        },
    )
}

impl Rig {
    /// Источник ответил: ответ лёг в ящик, толчок дошёл до движка (как `ThreeAWaker`), движок прочёл.
    pub(super) fn deliver_answer(&mut self, outside: &WhiteOutside, answer: WhiteAnswer) {
        outside.answers.publish(answer);
        self.engine.refresh_three_a(self.now);
        self.pump();
    }

    /// Результат кадра повторяющегося запроса через `after` от прошлого события.
    pub(super) fn frame_after(&mut self, after: Duration) {
        self.now += after;
        self.feed_result(result_meta(1, RequestOrigin::Repeating, None));
    }

    pub(super) fn white_status(&self) -> WhiteStatus {
        self.engine.status().three_a.white
    }

    pub(super) fn held(&self) -> HeldWhite {
        self.white_status().held
    }
}

/// Итоги заявок из событий.
pub(super) fn finished_requests(events: &[CameraEvent]) -> Vec<WhiteRequestResult> {
    events
        .iter()
        .filter_map(|event| match event {
            CameraEvent::WhiteRequestFinished { result } => Some(*result),
            _ => None,
        })
        .collect()
}

/// Смены белой из событий.
pub(super) fn white_changes(events: &[CameraEvent]) -> Vec<HeldWhite> {
    events
        .iter()
        .filter_map(|event| match event {
            CameraEvent::WhiteChanged { held } => Some(*held),
            _ => None,
        })
        .collect()
}
