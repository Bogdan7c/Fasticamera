//! Снимок состояния вида для экрана: приложение собирает его при каждом изменении и отдаёт экрану.
//!
//! Экран не видит ни библиотеку пресетов, ни конфиг модели вида, поэтому всё, что ему нужно показать
//! (список пресетов, что отличается от пресета, какие полосы и с каким цветом точки, что основа не умеет),
//! приходит одним сравниваемым значением. Приложение отслеживает изменения по `==` (watcher).

use crate::active::EditState;
use crate::bands::HueBandId;
use crate::control::{GlobalKnob, LookControl};
use crate::error::{PresetError, StorageFailure};
use crate::preset::{PresetEntry, PresetId};
use crate::recipe::LookRecipe;

/// Цвет точки полосы на экране. Считает приложение (из центра полосы модели вида), экран только рисует.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BandSwatch {
    /// Цвет в sRGB, 8 бит на канал.
    pub srgb: [u8; 3],
}

/// Полоса оттенка, как её показывает экран.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BandFacts {
    /// Идентификатор полосы.
    pub id: HueBandId,
    /// Цвет точки полосы.
    pub swatch: BandSwatch,
}

/// Что именно пошло не так (экран по варианту решает, что показать).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookProblemKind {
    /// Намерение отвергнуто библиотекой пресетов.
    Rejected(PresetError),
    /// Запись файла пресетов не удалась.
    SaveFailed(StorageFailure),
    /// Запись запрещена: файл не прочитан и затирать его нельзя.
    SavesRefused,
}

/// Последняя проблема. Серийный номер растёт с каждой новой проблемой: две одинаковые подряд
/// различимы по `==`, значит экран покажет вторую тоже.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookProblem {
    /// Номер проблемы (растёт на 1).
    pub serial: u64,
    /// Что случилось.
    pub kind: LookProblemKind,
}

/// Состояние вида для экрана.
#[derive(Debug, Clone, PartialEq)]
pub struct LookFacts {
    /// Активный пресет.
    pub active: PresetId,
    /// Список пресетов: сначала встроенные, затем свои.
    pub presets: Vec<PresetEntry>,
    /// Текущий рецепт (с правками).
    pub current: LookRecipe,
    /// Рецепт активного пресета без правок.
    pub preset: LookRecipe,
    /// Есть ли правки поверх пресета.
    pub edits: EditState,
    /// Полосы оттенка, которые знает модель вида.
    pub bands: Vec<BandFacts>,
    /// Ручки, которые текущая основа не умеет (экран показывает их приглушёнными).
    pub unsupported_knobs: Vec<GlobalKnob>,
    /// Последняя проблема, если была.
    pub problem: Option<LookProblem>,
}

impl LookFacts {
    /// Отличается ли орган управления от значения активного пресета.
    pub fn control_differs(&self, control: &LookControl) -> bool {
        self.current.differs_from(&self.preset, control)
    }

    /// Умеет ли текущая основа эту ручку.
    pub fn is_knob_supported(&self, knob: GlobalKnob) -> bool {
        !self.unsupported_knobs.contains(&knob)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knob::LookKnob;
    use crate::preset::BuiltinPreset;
    use crate::settings::LookSettings;
    use crate::tone_base::ToneBaseModel;

    fn facts(current: LookRecipe, unsupported_knobs: Vec<GlobalKnob>) -> LookFacts {
        let preset = LookRecipe {
            base: ToneBaseModel::Agx,
            settings: LookSettings::NEUTRAL,
        };
        LookFacts {
            active: PresetId::Builtin(BuiltinPreset::Honest),
            presets: Vec::new(),
            edits: if current == preset {
                EditState::Clean
            } else {
                EditState::EditedOnTop
            },
            current,
            preset,
            bands: Vec::new(),
            unsupported_knobs,
            problem: None,
        }
    }

    #[test]
    fn control_differs_reports_only_the_edited_control() {
        let edited = LookRecipe {
            base: ToneBaseModel::Aces,
            settings: LookSettings::NEUTRAL.with_knob(GlobalKnob::Contrast, LookKnob::new(0.4).unwrap()),
        };
        let facts = facts(edited, Vec::new());
        assert!(facts.control_differs(&LookControl::ToneBase));
        assert!(facts.control_differs(&LookControl::Knob(GlobalKnob::Contrast)));
        assert!(!facts.control_differs(&LookControl::Knob(GlobalKnob::Warmth)));
    }

    #[test]
    fn knob_support_follows_the_unsupported_list() {
        let clean = LookRecipe {
            base: ToneBaseModel::Agx,
            settings: LookSettings::NEUTRAL,
        };
        let facts = facts(clean, vec![GlobalKnob::Midtones]);
        assert!(!facts.is_knob_supported(GlobalKnob::Midtones));
        for knob in GlobalKnob::ALL.into_iter().filter(|knob| *knob != GlobalKnob::Midtones) {
            assert!(facts.is_knob_supported(knob), "{knob:?}");
        }
    }

    #[test]
    fn problems_with_different_serials_are_not_equal() {
        let first = LookProblem {
            serial: 1,
            kind: LookProblemKind::SavesRefused,
        };
        let second = LookProblem {
            serial: 2,
            kind: LookProblemKind::SavesRefused,
        };
        assert_ne!(first, second);
    }
}
