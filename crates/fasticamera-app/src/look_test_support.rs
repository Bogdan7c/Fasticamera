//! Общие помощники тестов вида картинки: подставное хранилище и готовые значения ручек.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use look_contract::{ColorWheel, ColorWheels, LookKnob, LookSettings, WheelPush, WheelZone};
use look_presets::{LibrarySnapshot, PresetStoreConfig, SaveOutcome, SaveRequestOutcome};

use crate::look_service::{LookHandle, LookPersistence, LookService, LookStartup};

/// Подставное хранилище: запоминает просьбы записи и отдаёт заранее заготовленные итоги.
#[derive(Clone)]
pub(crate) struct StubPersistence {
    pub requests: Arc<Mutex<Vec<LibrarySnapshot>>>,
    pub outcomes: Arc<Mutex<VecDeque<SaveOutcome>>>,
    /// Чем отвечать на просьбу записи.
    pub answer: SaveRequestOutcome,
}

impl StubPersistence {
    pub fn accepting() -> Self {
        Self {
            requests: Arc::default(),
            outcomes: Arc::default(),
            answer: SaveRequestOutcome::Queued,
        }
    }

    pub fn requested(&self) -> Vec<LibrarySnapshot> {
        self.requests.lock().expect("замок").clone()
    }

    pub fn push_outcome(&self, outcome: SaveOutcome) {
        self.outcomes.lock().expect("замок").push_back(outcome);
    }
}

impl LookPersistence for StubPersistence {
    fn request_save(&self, snapshot: LibrarySnapshot) -> SaveRequestOutcome {
        self.requests.lock().expect("замок").push(snapshot);
        self.answer
    }

    fn drain_outcomes(&self) -> Vec<SaveOutcome> {
        self.outcomes.lock().expect("замок").drain(..).collect()
    }
}

/// Ручка на библиотеку со встроенными пресетами из конфига по умолчанию и подставным хранилищем.
pub(crate) fn handle_with(persistence: StubPersistence) -> LookHandle {
    let config = PresetStoreConfig::default();
    let library = look_presets::PresetLibrary::new(config.builtin_catalog().expect("конфиг по умолчанию верен"));
    LookHandle::new(LookService::new(
        library,
        Box::new(persistence),
        LookStartup::InMemoryOnly,
    ))
}

pub(crate) fn knob(value: f32) -> LookKnob {
    LookKnob::new(value).expect("ручка в диапазоне")
}

/// Вид с одной заметной ручкой: по значению сразу видно, чей он.
pub(crate) fn contrast_look(value: f32) -> LookSettings {
    LookSettings {
        contrast: knob(value),
        ..LookSettings::NEUTRAL
    }
}

/// Вид только с колёсами: Lift к синему с тёмным кольцом, Gain к красному со светлым кольцом.
/// Ручки (контраст, теплота и прочие) нейтральны: отличие вида от нейтрали — только колёса.
pub(crate) fn wheels_look() -> LookSettings {
    let wheel = |hue_degrees: f32, ring: f32| ColorWheel {
        push: WheelPush::from_hue_degrees(hue_degrees, 0.8).expect("точка в круге"),
        ring: knob(ring),
    };
    LookSettings {
        wheels: ColorWheels::NEUTRAL
            .with_wheel(WheelZone::Lift, wheel(265.0, -0.3))
            .with_wheel(WheelZone::Gain, wheel(30.0, 0.5)),
        ..LookSettings::NEUTRAL
    }
}
