//! План хаптики: какой механизм вибромотора воспроизведёт iOS-паттерн на этом телефоне.
//!
//! Решение владельца (сессия 02): решает Rust, Kotlin только проигрывает готовую
//! [`HapticRealization`]. Цепочка запасных вариантов явная, отсутствие механизма — не молчание:
//! 1. примитив композиции, если телефон его поддерживает (на OnePlus 11 — нет, сессия 00);
//! 2. готовый эффект системы, если поддержка `YES`;
//! 3. своя амплитудная волна, если у мотора есть управление амплитудой (на OnePlus 11 — есть);
//! 4. готовый эффект с поддержкой `UNKNOWN` (система подставит свой запасной вариант);
//! 5. волна без управления амплитудой (только вкл/выкл);
//! 6. ничего не подошло или вибромотора нет — [`None`], сервис ответит `Unsupported`.
//!
//! Таблица рецептов ([`HapticTuning`]) — параметр, а не литералы в логике: её можно подстроить
//! по ощущениям владельца (smoke `platform` проигрывает все паттерны).

use shell_contract::haptics::{HapticPattern, HapticRealizationKind};

/// Готовые эффекты `VibrationEffect.EFFECT_*` (значения сверены с `android.jar` SDK 36).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PredefinedEffect {
    Click,
    DoubleClick,
    Tick,
    HeavyClick,
}

impl PredefinedEffect {
    pub const ALL: [PredefinedEffect; 4] = [
        PredefinedEffect::Click,
        PredefinedEffect::DoubleClick,
        PredefinedEffect::Tick,
        PredefinedEffect::HeavyClick,
    ];

    /// Константа Android, которую Kotlin передаёт в `createPredefined` без перевода.
    pub const fn android_code(self) -> i32 {
        match self {
            PredefinedEffect::Click => 0,
            PredefinedEffect::DoubleClick => 1,
            PredefinedEffect::Tick => 2,
            PredefinedEffect::HeavyClick => 5,
        }
    }
}

/// Примитивы `VibrationEffect.Composition.PRIMITIVE_*` (значения сверены с `android.jar` SDK 36).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompositionPrimitive {
    Click,
    Thud,
    Tick,
    LowTick,
}

impl CompositionPrimitive {
    pub const ALL: [CompositionPrimitive; 4] = [
        CompositionPrimitive::Click,
        CompositionPrimitive::Thud,
        CompositionPrimitive::Tick,
        CompositionPrimitive::LowTick,
    ];

    pub const fn android_code(self) -> i32 {
        match self {
            CompositionPrimitive::Click => 1,
            CompositionPrimitive::Thud => 2,
            CompositionPrimitive::Tick => 7,
            CompositionPrimitive::LowTick => 8,
        }
    }
}

/// Поддержка готового эффекта (`Vibrator.VIBRATION_EFFECT_SUPPORT_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectSupport {
    Yes,
    No,
    /// Железо не сообщает; система при воспроизведении подставит свой вариант.
    Unknown,
}

impl EffectSupport {
    /// Из кода Android: 0 — UNKNOWN, 1 — YES, 2 — NO; неизвестный код честно считаем UNKNOWN.
    pub fn from_android_code(code: i32) -> Self {
        match code {
            1 => EffectSupport::Yes,
            2 => EffectSupport::No,
            _ => EffectSupport::Unknown,
        }
    }
}

/// Возможности вибромотора, опрошенные у платформы при подключении Activity.
#[derive(Debug, Clone, PartialEq)]
pub struct HapticCapabilities {
    pub has_vibrator: bool,
    pub has_amplitude_control: bool,
    pub primitives: Vec<(CompositionPrimitive, bool)>,
    pub effects: Vec<(PredefinedEffect, EffectSupport)>,
}

impl HapticCapabilities {
    /// Телефон без вибромотора (или Activity ещё не опрошена).
    pub fn absent() -> Self {
        Self {
            has_vibrator: false,
            has_amplitude_control: false,
            primitives: Vec::new(),
            effects: Vec::new(),
        }
    }

    fn primitive_supported(&self, primitive: CompositionPrimitive) -> bool {
        self.primitives
            .iter()
            .any(|&(candidate, supported)| candidate == primitive && supported)
    }

    fn effect_support(&self, effect: PredefinedEffect) -> EffectSupport {
        self.effects
            .iter()
            .find(|(candidate, _)| *candidate == effect)
            .map_or(EffectSupport::No, |&(_, support)| support)
    }
}

/// Шаг амплитудной волны: длительность и амплитуда 0–255 (0 — пауза).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaveformStep {
    pub duration_ms: u32,
    pub amplitude: u8,
}

/// Готовый способ воспроизведения для Kotlin.
#[derive(Debug, Clone, PartialEq)]
pub enum HapticRealization {
    Primitive {
        primitive: CompositionPrimitive,
        /// Сила 0.0–1.0 (`Composition.addPrimitive(id, scale)`).
        scale: f32,
    },
    PredefinedEffect(PredefinedEffect),
    Waveform(Vec<WaveformStep>),
}

impl HapticRealization {
    pub fn kind(&self) -> HapticRealizationKind {
        match self {
            HapticRealization::Primitive { .. } => HapticRealizationKind::Primitive,
            HapticRealization::PredefinedEffect(_) => HapticRealizationKind::PredefinedEffect,
            HapticRealization::Waveform(_) => HapticRealizationKind::Waveform,
        }
    }
}

/// Рецепт одного паттерна: предпочтительные механизмы по убыванию «родства» с iOS.
#[derive(Debug, Clone, PartialEq)]
pub struct HapticRecipe {
    pub primitive: Option<(CompositionPrimitive, f32)>,
    pub predefined: Option<PredefinedEffect>,
    pub waveform: Vec<WaveformStep>,
}

/// Таблица рецептов всех паттернов.
#[derive(Debug, Clone, PartialEq)]
pub struct HapticTuning {
    recipes: Vec<(HapticPattern, HapticRecipe)>,
}

/// Короткий конструктор шага волны для таблицы по умолчанию.
const fn step(duration_ms: u32, amplitude: u8) -> WaveformStep {
    WaveformStep { duration_ms, amplitude }
}

impl Default for HapticTuning {
    /// Первое приближение «как iOS»: щелчки короткие (8–25 мс), сила растёт от selection к
    /// heavy, уведомления — двойные/тройные импульсы. Подстраивается по ощущениям владельца.
    fn default() -> Self {
        use CompositionPrimitive as Primitive;
        use HapticPattern as Pattern;
        use PredefinedEffect as Effect;
        let recipe = |primitive, predefined, waveform: &[WaveformStep]| HapticRecipe {
            primitive,
            predefined,
            waveform: waveform.to_vec(),
        };
        Self {
            recipes: vec![
                (
                    Pattern::SelectionTick,
                    recipe(Some((Primitive::Tick, 0.5)), Some(Effect::Tick), &[step(8, 70)]),
                ),
                (
                    Pattern::LightImpact,
                    recipe(Some((Primitive::Tick, 0.9)), Some(Effect::Tick), &[step(12, 110)]),
                ),
                (
                    Pattern::MediumImpact,
                    recipe(Some((Primitive::Click, 0.7)), Some(Effect::Click), &[step(18, 170)]),
                ),
                (
                    Pattern::HeavyImpact,
                    recipe(
                        Some((Primitive::Click, 1.0)),
                        Some(Effect::HeavyClick),
                        &[step(25, 255)],
                    ),
                ),
                (
                    Pattern::SoftImpact,
                    recipe(Some((Primitive::LowTick, 0.8)), Some(Effect::Tick), &[step(20, 70)]),
                ),
                (
                    Pattern::RigidImpact,
                    recipe(Some((Primitive::Click, 1.0)), Some(Effect::Click), &[step(10, 230)]),
                ),
                (
                    Pattern::Success,
                    recipe(
                        None,
                        Some(Effect::DoubleClick),
                        &[step(15, 150), step(70, 0), step(15, 220)],
                    ),
                ),
                (
                    Pattern::Warning,
                    recipe(
                        None,
                        Some(Effect::DoubleClick),
                        &[step(20, 220), step(90, 0), step(15, 120)],
                    ),
                ),
                (
                    Pattern::Error,
                    recipe(
                        None,
                        Some(Effect::HeavyClick),
                        &[step(15, 200), step(50, 0), step(15, 200), step(50, 0), step(25, 255)],
                    ),
                ),
            ],
        }
    }
}

impl HapticTuning {
    /// Рецепт паттерна; паттерн без рецепта — пустой рецепт (план даст `None`, не панику).
    pub fn recipe(&self, pattern: HapticPattern) -> HapticRecipe {
        self.recipes.iter().find(|(candidate, _)| *candidate == pattern).map_or(
            HapticRecipe {
                primitive: None,
                predefined: None,
                waveform: Vec::new(),
            },
            |(_, recipe)| recipe.clone(),
        )
    }
}

/// Выбирает способ воспроизведения паттерна по цепочке из документации модуля.
pub fn plan_haptic(
    pattern: HapticPattern,
    capabilities: &HapticCapabilities,
    tuning: &HapticTuning,
) -> Option<HapticRealization> {
    if !capabilities.has_vibrator {
        return None;
    }
    let recipe = tuning.recipe(pattern);
    if let Some((primitive, scale)) = recipe.primitive
        && capabilities.primitive_supported(primitive)
    {
        return Some(HapticRealization::Primitive { primitive, scale });
    }
    let predefined_support = recipe
        .predefined
        .map(|effect| (effect, capabilities.effect_support(effect)));
    if let Some((effect, EffectSupport::Yes)) = predefined_support {
        return Some(HapticRealization::PredefinedEffect(effect));
    }
    let has_waveform = !recipe.waveform.is_empty();
    if has_waveform && capabilities.has_amplitude_control {
        return Some(HapticRealization::Waveform(recipe.waveform));
    }
    if let Some((effect, EffectSupport::Unknown)) = predefined_support {
        return Some(HapticRealization::PredefinedEffect(effect));
    }
    if has_waveform {
        return Some(HapticRealization::Waveform(recipe.waveform));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Возможности OnePlus 11 по сессии 00: примитивов нет, амплитуда есть.
    fn oneplus_capabilities(effect_support: EffectSupport) -> HapticCapabilities {
        HapticCapabilities {
            has_vibrator: true,
            has_amplitude_control: true,
            primitives: CompositionPrimitive::ALL
                .iter()
                .map(|&primitive| (primitive, false))
                .collect(),
            effects: PredefinedEffect::ALL
                .iter()
                .map(|&effect| (effect, effect_support))
                .collect(),
        }
    }

    #[test]
    fn supported_primitive_is_preferred() {
        let mut capabilities = oneplus_capabilities(EffectSupport::Yes);
        capabilities.primitives = vec![(CompositionPrimitive::Tick, true)];
        assert_eq!(
            plan_haptic(HapticPattern::SelectionTick, &capabilities, &HapticTuning::default()),
            Some(HapticRealization::Primitive {
                primitive: CompositionPrimitive::Tick,
                scale: 0.5
            })
        );
    }

    #[test]
    fn missing_primitive_falls_back_to_supported_effect() {
        let plan = plan_haptic(
            HapticPattern::HeavyImpact,
            &oneplus_capabilities(EffectSupport::Yes),
            &HapticTuning::default(),
        );
        assert_eq!(
            plan,
            Some(HapticRealization::PredefinedEffect(PredefinedEffect::HeavyClick))
        );
    }

    #[test]
    fn unknown_effect_support_prefers_own_waveform_when_amplitude_controlled() {
        let plan = plan_haptic(
            HapticPattern::MediumImpact,
            &oneplus_capabilities(EffectSupport::Unknown),
            &HapticTuning::default(),
        );
        assert_eq!(plan, Some(HapticRealization::Waveform(vec![step(18, 170)])));
    }

    #[test]
    fn unknown_effect_is_used_when_no_amplitude_control() {
        let mut capabilities = oneplus_capabilities(EffectSupport::Unknown);
        capabilities.has_amplitude_control = false;
        let plan = plan_haptic(HapticPattern::LightImpact, &capabilities, &HapticTuning::default());
        assert_eq!(plan, Some(HapticRealization::PredefinedEffect(PredefinedEffect::Tick)));
    }

    #[test]
    fn unsupported_effects_without_amplitude_still_vibrate_on_off() {
        let mut capabilities = oneplus_capabilities(EffectSupport::No);
        capabilities.has_amplitude_control = false;
        let plan = plan_haptic(HapticPattern::Success, &capabilities, &HapticTuning::default());
        assert!(matches!(plan, Some(HapticRealization::Waveform(steps)) if steps.len() == 3));
    }

    #[test]
    fn every_pattern_has_a_realization_on_oneplus() {
        let tuning = HapticTuning::default();
        for support in [EffectSupport::Yes, EffectSupport::No, EffectSupport::Unknown] {
            let capabilities = oneplus_capabilities(support);
            for pattern in HapticPattern::ALL {
                assert!(
                    plan_haptic(pattern, &capabilities, &tuning).is_some(),
                    "{pattern:?} без реализации при поддержке {support:?}"
                );
            }
        }
    }

    #[test]
    fn no_vibrator_means_none_not_silence() {
        assert_eq!(
            plan_haptic(
                HapticPattern::Error,
                &HapticCapabilities::absent(),
                &HapticTuning::default()
            ),
            None
        );
    }

    #[test]
    fn empty_recipe_yields_none() {
        let tuning = HapticTuning { recipes: Vec::new() };
        assert_eq!(
            plan_haptic(
                HapticPattern::SelectionTick,
                &oneplus_capabilities(EffectSupport::Yes),
                &tuning
            ),
            None
        );
    }

    #[test]
    fn android_codes_match_sdk_36() {
        // Значения из `javap -constants android.jar` (SDK 36): ошибка здесь подменила бы эффект.
        assert_eq!(PredefinedEffect::HeavyClick.android_code(), 5);
        assert_eq!(PredefinedEffect::Tick.android_code(), 2);
        assert_eq!(CompositionPrimitive::Tick.android_code(), 7);
        assert_eq!(CompositionPrimitive::LowTick.android_code(), 8);
        assert_eq!(EffectSupport::from_android_code(1), EffectSupport::Yes);
        assert_eq!(EffectSupport::from_android_code(2), EffectSupport::No);
        assert_eq!(EffectSupport::from_android_code(0), EffectSupport::Unknown);
    }
}
