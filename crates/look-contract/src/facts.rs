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

/// Минимум стопов кольца: меньше — ободок распадается на видимые сектора, а не на плавный круг оттенков.
pub const MIN_WHEEL_RING_STOPS: usize = 12;

/// Почему набор стопов кольца отвергнут.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WheelHueRingError {
    /// Стопов меньше [`MIN_WHEEL_RING_STOPS`].
    #[error("в кольце оттенков {count} стопов: нужно не меньше {MIN_WHEEL_RING_STOPS}")]
    TooFewStops {
        /// Сколько стопов передали.
        count: usize,
    },
}

/// Цвета ободка колеса: круг оттенков, по которому экран рисует кольцо вокруг точки толчка. Считает приложение
/// (экран не умеет OKLab), экран только красит.
///
/// **Инвариант:** стоп `i` из `N` — оттенок OKLCh `i · 360 / N` градусов, где 0° = `+a` (вправо),
/// угол растёт ПРОТИВ часовой стрелки (90° = `+b`, вверх), как на самом круге толчка ([`crate::WheelPush`]):
/// ободок обязан совпадать с направлением, в которое точка толкает цвет. Последний стоп `N − 1` — это
/// `(N − 1) · 360 / N` градусов; 360° = 0° в списке отдельно не повторяется (экран замыкает круг сам).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WheelHueRing {
    stops: Vec<BandSwatch>,
}

impl WheelHueRing {
    /// Проверяет число стопов и строит кольцо.
    pub fn new(stops: Vec<BandSwatch>) -> Result<Self, WheelHueRingError> {
        if stops.len() < MIN_WHEEL_RING_STOPS {
            return Err(WheelHueRingError::TooFewStops { count: stops.len() });
        }
        Ok(Self { stops })
    }

    /// Кольцо одного цвета (минимальное число стопов): значение «цвета ещё не посчитаны» для фактов без источника
    /// цвета и для тестов экрана, которым оттенки не важны.
    pub fn solid(color: BandSwatch) -> Self {
        Self {
            stops: vec![color; MIN_WHEEL_RING_STOPS],
        }
    }

    /// Стопы по кругу, начиная с 0° (`+a`), против часовой стрелки.
    pub fn stops(&self) -> &[BandSwatch] {
        &self.stops
    }

    /// Оттенок OKLCh стопа `index` в градусах: `index · 360 / N`. Единый источник формулы для приложения и экрана.
    pub fn hue_degrees_of_stop(stop_count: usize, index: usize) -> f32 {
        index as f32 * 360.0 / stop_count as f32
    }
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
    /// Цвета ободка колёс цвета (одинаковы у всех четырёх колёс).
    pub wheel_ring: WheelHueRing,
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
            wheel_ring: WheelHueRing::solid(BandSwatch { srgb: [128; 3] }),
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

#[cfg(test)]
mod wheel_ring_tests {
    use super::*;

    const GRAY: BandSwatch = BandSwatch { srgb: [128; 3] };

    #[test]
    fn ring_needs_the_minimum_number_of_stops() {
        assert_eq!(
            WheelHueRing::new(vec![GRAY; MIN_WHEEL_RING_STOPS - 1]),
            Err(WheelHueRingError::TooFewStops {
                count: MIN_WHEEL_RING_STOPS - 1
            })
        );
        assert_eq!(
            WheelHueRing::new(Vec::new()),
            Err(WheelHueRingError::TooFewStops { count: 0 })
        );
        let ring = WheelHueRing::new(vec![GRAY; MIN_WHEEL_RING_STOPS]).unwrap();
        assert_eq!(ring.stops().len(), MIN_WHEEL_RING_STOPS);
    }

    #[test]
    fn solid_ring_is_valid_and_single_coloured() {
        let ring = WheelHueRing::solid(GRAY);
        assert_eq!(ring.stops().len(), MIN_WHEEL_RING_STOPS);
        assert!(ring.stops().iter().all(|stop| *stop == GRAY));
    }

    #[test]
    fn stop_hues_run_counter_clockwise_from_plus_a_without_repeating_360() {
        let count = 72;
        assert_eq!(WheelHueRing::hue_degrees_of_stop(count, 0), 0.0);
        assert_eq!(
            WheelHueRing::hue_degrees_of_stop(count, 18),
            90.0,
            "четверть круга = +b"
        );
        assert_eq!(WheelHueRing::hue_degrees_of_stop(count, 36), 180.0);
        assert_eq!(WheelHueRing::hue_degrees_of_stop(count, 71), 355.0);
    }
}
