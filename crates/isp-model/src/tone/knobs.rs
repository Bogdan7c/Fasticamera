//! Единый шов ручек тона (решение 182): основа тона — сменная МОДЕЛЬ, а ручки тона (контраст, средние тона,
//! тени, света) не прибиты к AgX.
//!
//! Слой вида ([`crate::look`]) переводит значения ручек в [`ToneKnobAmounts`] — числа в единицах, не зависящих от
//! модели («сколько ступеней наклона», «сколько ступеней серого экрана»), и больше ничего о модели не знает.
//! Каждая модель реализует [`ToneModelSettings::with_knob`]: как ОДНА ручка меняет её настройки, либо
//! [`ToneKnobUnsupported`] — «эта модель такой ручки не поддерживает» (различимый исход, не паника и не молчание).
//! Новая модель = свой тип настроек + эта реализация + вариант в [`super::base::ToneBase`]; модуль ручек и стадия
//! цвета вида не правятся (см. «как добавить модель» в [`super::base`]).

use super::ToneCurveSettings;
use super::agx_settings::AgxSettings;

/// Ручка тона.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToneKnob {
    /// Контраст: наклон кривой на сером.
    Contrast,
    /// Средние тона: во что превращается серый сцены на экране.
    Midtones,
    /// Тени: форма носка (`+` — тени светлее).
    Shadows,
    /// Света: форма плеча (`+` — света светлее).
    Highlights,
}

/// Название модели основы (для сообщений и отчёта разрешения).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToneModelKind {
    /// Прежняя лог-логистическая кривая.
    Legacy,
    /// Основа AgX.
    Agx,
    /// Основа filmic (своя реализация идеи darktable filmic RGB).
    Filmic,
    /// Основа ACES (RRT и выходное преобразование SDR 100 нит, Academy).
    Aces,
}

/// Модель основы не поддерживает эту ручку.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("основа {model:?} не поддерживает ручку {knob:?}")]
pub struct ToneKnobUnsupported {
    pub knob: ToneKnob,
    pub model: ToneModelKind,
}

/// Значения ручек тона в единицах, не зависящих от модели. Нуль — «не трогать».
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ToneKnobAmounts {
    /// Контраст: `log2` множителя наклона.
    pub contrast_log2: f32,
    /// Средние тона: сдвиг серого экрана в ступенях (`+` — светлее).
    pub midtones_ev: f32,
    /// Тени: сила формы носка со знаком (`+` — светлее; единицы — по настройке модели вида).
    pub shadows: f32,
    /// Света: сила формы плеча со знаком (`+` — светлее).
    pub highlights: f32,
}

impl ToneKnobAmounts {
    /// Все четыре ручки в нуле.
    pub fn is_neutral(&self) -> bool {
        self.pairs().iter().all(|(_, amount)| *amount == 0.0)
    }

    /// Ручки по порядку применения.
    pub fn pairs(&self) -> [(ToneKnob, f32); 4] {
        [
            (ToneKnob::Contrast, self.contrast_log2),
            (ToneKnob::Midtones, self.midtones_ev),
            (ToneKnob::Shadows, self.shadows),
            (ToneKnob::Highlights, self.highlights),
        ]
    }
}

/// Контракт модели основы: как ручка меняет её настройки.
pub trait ToneModelSettings: Sized {
    /// Какая это модель (для сообщения об отказе).
    const KIND: ToneModelKind;

    /// Настройки с применённой ручкой `knob` силой `amount ≠ 0`. Нейтральные ручки сюда не приходят
    /// (нейтраль = настройки без изменений бит в бит, это гарантирует вызывающий).
    fn with_knob(&self, knob: ToneKnob, amount: f32) -> Result<Self, ToneKnobUnsupported>;
}

/// Итог применения набора ручек к настройкам модели.
#[derive(Debug, Clone, PartialEq)]
pub struct KnobOutcome<S> {
    /// Настройки с применёнными поддержанными ручками.
    pub settings: S,
    /// Ненулевые ручки, которых эта модель не поддерживает (они не применены).
    pub unsupported: Vec<ToneKnobUnsupported>,
}

/// Применяет все ненулевые ручки; неподдержанные не прерывают разрешение, а попадают в отчёт.
pub fn apply_knobs_lenient<S: ToneModelSettings + Clone>(base: &S, amounts: &ToneKnobAmounts) -> KnobOutcome<S> {
    let mut settings = base.clone();
    let mut unsupported = Vec::new();
    for (knob, amount) in amounts.pairs() {
        if amount == 0.0 {
            continue;
        }
        match settings.with_knob(knob, amount) {
            Ok(adjusted) => settings = adjusted,
            Err(refusal) => unsupported.push(refusal),
        }
    }
    KnobOutcome { settings, unsupported }
}

/// То же, но первая неподдержанная ручка — ошибка (для проверки конфига на краях).
pub fn apply_knobs_strict<S: ToneModelSettings + Clone>(
    base: &S,
    amounts: &ToneKnobAmounts,
) -> Result<S, ToneKnobUnsupported> {
    let mut settings = base.clone();
    for (knob, amount) in amounts.pairs() {
        if amount != 0.0 {
            settings = settings.with_knob(knob, amount)?;
        }
    }
    Ok(settings)
}

impl ToneModelSettings for AgxSettings {
    const KIND: ToneModelKind = ToneModelKind::Agx;

    /// AgX: контраст — наклон `s`, средние тона — серый экрана, тени — степень носка `p0`, света — степень
    /// плеча `p1` (вторая кривая не нужна). Направления проверены замером (тест `shadows_and_highlights_...`):
    /// меньшая степень носка `p0` делает тени СВЕТЛЕЕ (степень делится на `2^amount`), большая степень плеча `p1`
    /// делает света СВЕТЛЕЕ (степень умножается). В `e = 0` значение и наклон не меняются (C1 в нуле).
    fn with_knob(&self, knob: ToneKnob, amount: f32) -> Result<Self, ToneKnobUnsupported> {
        let scale = amount.exp2();
        Ok(match knob {
            ToneKnob::Contrast => Self {
                slope: self.slope * scale,
                ..*self
            },
            ToneKnob::Midtones => Self {
                display_middle_gray: self.display_middle_gray * scale,
                ..*self
            },
            ToneKnob::Shadows => Self {
                toe_power: self.toe_power / scale,
                ..*self
            },
            ToneKnob::Highlights => Self {
                shoulder_power: self.shoulder_power * scale,
                ..*self
            },
        })
    }
}

impl ToneModelSettings for ToneCurveSettings {
    const KIND: ToneModelKind = ToneModelKind::Legacy;

    /// Прежняя кривая: контраст — показатель `c`, средние тона — серый экрана. Носка и плеча у неё нет
    /// (одна лог-логистическая форма): тени и света — [`ToneKnobUnsupported`].
    fn with_knob(&self, knob: ToneKnob, amount: f32) -> Result<Self, ToneKnobUnsupported> {
        let scale = amount.exp2();
        match knob {
            ToneKnob::Contrast => Ok(Self {
                contrast: self.contrast * scale,
                ..*self
            }),
            ToneKnob::Midtones => Ok(Self {
                display_middle_gray: self.display_middle_gray * scale,
                ..*self
            }),
            ToneKnob::Shadows | ToneKnob::Highlights => Err(ToneKnobUnsupported {
                knob,
                model: Self::KIND,
            }),
        }
    }
}
