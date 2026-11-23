//! Помощники тестов раздела калибровки: факты о виде, экран в режиме CALIBRATE, ввод номера запроса.

use look_contract::{
    BuiltinPreset, EditState, HueBandId, LookFacts, LookKnob, LookRecipe, LookSettings, PresetEntry, PresetId,
    PresetKind, PresetName, ToneBaseModel, UserPresetId,
};
use shell_contract::orientation::DeviceOrientation;

use crate::action::ScreenAction;
use crate::effect::Effect;
use crate::modes::CaptureMode;
use crate::test_support::Screen;

/// Свой пресет «Sunset» (номер 7).
pub(crate) const SUNSET: UserPresetId = UserPresetId(7);
/// Свой пресет «Night» (номер 9).
pub(crate) const NIGHT: UserPresetId = UserPresetId(9);

/// Число из литерала теста в допустимых пределах ручки.
pub(crate) fn knob(value: f32) -> LookKnob {
    LookKnob::new(value).expect("значение ручки теста в диапазоне")
}

/// Идентификатор полосы из литерала теста.
pub(crate) fn band(id: &str) -> HueBandId {
    HueBandId::new(id).expect("идентификатор полосы теста допустим")
}

fn entry(id: PresetId, name: &str, kind: PresetKind) -> PresetEntry {
    PresetEntry {
        id,
        name: PresetName::new(name).expect("имя теста допустимо"),
        kind,
    }
}

/// Факты: активен встроенный «As Eye» без правок, основа AgX, нейтральные ручки, три полосы (red, sky, green),
/// два своих пресета (Sunset, Night), неподдержанных ручек и проблем нет.
pub(crate) fn look_facts() -> LookFacts {
    let recipe = LookRecipe {
        base: ToneBaseModel::Agx,
        settings: LookSettings::NEUTRAL,
    };
    LookFacts {
        active: PresetId::Builtin(BuiltinPreset::AsEye),
        presets: vec![
            entry(PresetId::Builtin(BuiltinPreset::Honest), "Honest", PresetKind::Builtin),
            entry(PresetId::Builtin(BuiltinPreset::AsEye), "As Eye", PresetKind::Builtin),
            entry(PresetId::User(SUNSET), "Sunset", PresetKind::User),
            entry(PresetId::User(NIGHT), "Night", PresetKind::User),
        ],
        current: recipe.clone(),
        preset: recipe,
        edits: EditState::Clean,
        bands: ["red", "sky", "green"]
            .into_iter()
            .map(|id| look_contract::BandFacts {
                id: band(id),
                swatch: look_contract::BandSwatch { srgb: [200, 40, 40] },
            })
            .collect(),
        wheel_ring: test_wheel_ring(),
        unsupported_knobs: Vec::new(),
        problem: None,
    }
}

/// Кольцо оттенков теста: не математика OKLab, а просто разные цвета у соседних стопов, чтобы растр кольца в тестах
/// был различим по пикселям. Число стопов — 36 (шаг 10°).
pub(crate) fn test_wheel_ring() -> look_contract::WheelHueRing {
    const STOP_COUNT: u8 = 36;
    let stops = (0..STOP_COUNT)
        .map(|index| look_contract::BandSwatch {
            srgb: [index * 7, 255 - index * 7, 128],
        })
        .collect();
    look_contract::WheelHueRing::new(stops).expect("в кольце теста достаточно стопов")
}

/// Факты, в которых активен свой пресет `preset` (правок нет).
pub(crate) fn look_facts_with_active(preset: PresetId) -> LookFacts {
    LookFacts {
        active: preset,
        ..look_facts()
    }
}

/// Факты с правкой: текущие настройки заменены на `edited`, пресет остался прежним.
pub(crate) fn look_facts_edited(edited: LookSettings) -> LookFacts {
    let mut facts = look_facts();
    facts.current.settings = edited;
    facts.edits = EditState::EditedOnTop;
    facts
}

impl Screen {
    /// Экран с размером surface, живой камерой и фактами о виде (режим PHOTO).
    pub(crate) fn with_look(facts: LookFacts) -> Self {
        let mut screen = Self::ready();
        screen.step(ScreenAction::LookFactsChanged(Box::new(facts)));
        screen
    }

    /// То же, но в режиме CALIBRATE, телефон держат портретно.
    pub(crate) fn calibrating(facts: LookFacts) -> Self {
        let mut screen = Self::with_look(facts);
        screen.step(ScreenAction::ModeSelected(CaptureMode::Calibrate));
        screen
    }

    /// Новый снимок фактов поверх прежних.
    pub(crate) fn push_look(&mut self, facts: LookFacts) {
        self.step(ScreenAction::LookFactsChanged(Box::new(facts)));
    }

    /// Поворот телефона.
    pub(crate) fn rotate_to(&mut self, orientation: DeviceOrientation) -> crate::reduce::ScreenTransition {
        self.step(ScreenAction::DeviceOrientationChanged(orientation))
    }
}

/// Эффекты `Effect::Look` из списка (по порядку).
pub(crate) fn look_intents(effects: &[Effect]) -> Vec<&look_contract::LookIntent> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Look(intent) => Some(intent),
            _ => None,
        })
        .collect()
}
