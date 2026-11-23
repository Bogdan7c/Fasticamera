//! Секция `viewfinder.tone`: ручки тона видоискателя до сессии 14a (решение 98, сессия 12b).
//!
//! # Что здесь можно крутить, а что нет
//! Только то, что меняет вид картинки на экране, но не экспозицию:
//! - `contrast` — крутизна S-кривой в середине (1 — без S-образности);
//! - `display_middle_gray` — насколько светлой выходит на экране середина (средний серый сцены).
//!
//! Средний серый **сцены** (`scene_middle_gray`) сюда намеренно не вынесен: это цель нашего AE (08a).
//! Ручка на нём заставила бы AE пересчитать выдержку, а экспозицию уже крутит EV. Поэтому
//! [`ViewfinderToneConfig::curve_settings`] всегда берёт его, белый сцены и сохранение оттенка из
//! умолчания `isp-model` — тот же источник, что у цели AE (`IspParamsCache::base_scene_middle_gray`).
//!
//! # Почему проверка учитывает AE
//! Кривая строится для каждого кадра с остаточной подтяжкой AE (`k` в пределах ±`max_residual_ev`):
//! подтяжка сдвигает средний серый сцены (`scene_middle_gray / k`). Сочетание контраста и средних тонов,
//! которое строится при `k = 1`, может не строиться при крайнем `k` — тогда ISP молча показывал бы
//! прежние параметры. Поэтому секция проверяется при старте на обоих краях подтяжки и в середине:
//! плохое значение — ошибка запуска с именем поля, а не тихий отказ на каждом кадре.
//!
//! # Основа тона (14a, решения 101, 176, 182)
//! `base` выбирает модель основы: `"agx"` (по умолчанию), `"aces"`, `"filmic"` или `"legacy"` (прежняя S-кривая,
//! вариант сравнения до 14b). У каждой модели своя секция настроек — `agx`, `aces`, `filmic` (формат — как у
//! `isp-model`; пропущенное поле и пропущенная секция берут умолчание модели): через них владелец крутит
//! параметры конкретной модели в 14b без пересборки. Прежняя кривая своей секции не имеет: её числа — ручки ниже.
//!
//! Две общие ручки — `contrast` и `display_middle_gray` — применяются к ВЫБРАННОЙ модели одним правилом, чтобы
//! видоискатель и снимок разрешали одно и то же. Ручка в умолчании (`1,35` и `0,20`) модель НЕ трогает: берётся
//! секция модели как есть. Иначе:
//! - `contrast` — у прежней основы показатель S-кривой; у AgX и filmic он масштабирует наклон на сером
//!   ПРОПОРЦИОНАЛЬНО (`slope = slope_секции · contrast / contrast_по_умолчанию`), у ACES — множитель оси
//!   экспозиции (`contrast_секции · contrast / contrast_по_умолчанию`). Вдвое больший контраст — вдвое круче;
//! - `display_middle_gray` — серый экрана у любой модели: ручка ПЕРЕКРЫВАЕТ `display_middle_gray` секции модели.
//!
//! `scene_middle_gray` в секциях моделей — цель AE (см. выше), поэтому он обязан совпадать с умолчанием; иное
//! значение — ошибка запуска [`ToneConfigError::SceneMiddleGrayIsAeTarget`], а не тихое расхождение экспозиции.
//!
//! Проверка при старте идёт для ВСЕХ моделей и на краях подтяжки AE (выбор можно сменить без правки остального
//! конфига: смена `base` не должна неожиданно ронять запуск).
//!
//! # Поддержка на GPU
//! Все модели основы (agx, aces, filmic, legacy) считаются на GPU в видоискателе, качественном пути и снимке, поэтому
//! выбор любой из них проходит проверку запуска; отдельной проверки «модель есть на GPU» больше нет.

use isp_model::exposure::ExposureGain;
use isp_model::image::DisplayGamut;
use isp_model::tone::aces::{AcesSettings, InvalidAces};
use isp_model::tone::agx_settings::{AgxSettings, InvalidAgx};
use isp_model::tone::base::{InvalidToneBase, ToneBase};
use isp_model::tone::filmic::{FilmicSettings, InvalidFilmic};
use isp_model::tone::knobs::ToneModelKind;
use isp_model::tone::{InvalidToneCurve, ToneCurve, ToneCurveSettings};
use look_contract::ToneBaseModel;
use serde::{Deserialize, Serialize};

/// Какая основа тона формирует картинку видоискателя и снимка (см. модуль).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToneBaseChoice {
    /// AgX в рабочем пространстве Rec. 2020 со своим белым точки (решения 101, 176, 177).
    #[default]
    Agx,
    /// ACES: RRT и выходное преобразование SDR 100 нит Academy (решение 182). Считается на GPU во всех путях.
    Aces,
    /// filmic: своя реализация идеи darktable filmic RGB (решение 182). Считается на GPU во всех путях.
    Filmic,
    /// Прежняя S-кривая: вариант сравнения до 14b.
    Legacy,
}

/// Основа конфига ↔ основа словаря вида (решение 195): модели те же, перечисление в словаре нужно экрану и
/// файлу пресетов, которым конфиг рендера не виден. Единственное место соответствия.
impl From<ToneBaseChoice> for ToneBaseModel {
    fn from(choice: ToneBaseChoice) -> Self {
        match choice {
            ToneBaseChoice::Agx => Self::Agx,
            ToneBaseChoice::Aces => Self::Aces,
            ToneBaseChoice::Filmic => Self::Filmic,
            ToneBaseChoice::Legacy => Self::Legacy,
        }
    }
}

impl ToneBaseChoice {
    /// Какая это модель (для сообщений и проверки поддержки на GPU).
    pub fn model_kind(self) -> ToneModelKind {
        match self {
            Self::Agx => ToneModelKind::Agx,
            Self::Aces => ToneModelKind::Aces,
            Self::Filmic => ToneModelKind::Filmic,
            Self::Legacy => ToneModelKind::Legacy,
        }
    }
}

/// Ручки тона видоискателя (см. модуль).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ViewfinderToneConfig {
    /// Основа тона. По умолчанию AgX.
    pub base: ToneBaseChoice,
    /// Контраст S-кривой (> 0). Правило применения к модели — в доке модуля.
    pub contrast: f32,
    /// Линейная яркость экрана (0…1), в которую превращается средний серый сцены.
    pub display_middle_gray: f32,
    /// Настройки модели AgX (умолчание — умолчание модели).
    pub agx: AgxSettings,
    /// Настройки модели ACES (умолчание — умолчание модели).
    pub aces: AcesSettings,
    /// Настройки модели filmic (умолчание — умолчание модели).
    pub filmic: FilmicSettings,
}

impl Default for ViewfinderToneConfig {
    /// Умолчание `isp-model` (оценка сессии 06): контраст 1,35, середина экрана 0,20.
    fn default() -> Self {
        let model = ToneCurveSettings::default();
        Self {
            base: ToneBaseChoice::default(),
            contrast: model.contrast,
            display_middle_gray: model.display_middle_gray,
            agx: AgxSettings::default(),
            aces: AcesSettings::default(),
            filmic: FilmicSettings::default(),
        }
    }
}

/// Подтяжки AE, на которых проверяется строимость: без подтяжки и оба края предела.
fn residual_edges(max_residual_ev: f32) -> [f32; 3] {
    [0.0, -max_residual_ev, max_residual_ev]
}

/// Множитель подтяжки для `residual_ev`; не конечный — ошибка предела (проверить тон на краях нельзя).
fn exposure_gain_at(residual_ev: f32, max_residual_ev: f32) -> Result<(f32, ExposureGain), ToneConfigError> {
    ExposureGain::from_ev(residual_ev)
        .map(|gain| (residual_ev, gain))
        .map_err(|_| ToneConfigError::ResidualLimitUnusable { max_residual_ev })
}

/// Секция тона не годится.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum ToneConfigError {
    /// Кривая с такими ручками не строится (при указанной остаточной подтяжке AE).
    #[error("viewfinder.tone: {source} (при остаточной подтяжке AE {residual_ev:+.2} EV)")]
    CurveUnusable {
        /// При какой подтяжке AE кривая не строится, EV.
        residual_ev: f32,
        /// Что именно не так с кривой.
        #[source]
        source: InvalidToneCurve,
    },
    /// Основа AgX с такими ручками не строится (при указанной остаточной подтяжке AE).
    #[error("viewfinder.tone (основа AgX): {source} (при остаточной подтяжке AE {residual_ev:+.2} EV)")]
    AgxUnusable {
        /// При какой подтяжке AE основа не строится, EV.
        residual_ev: f32,
        /// Что именно не так с основой.
        #[source]
        source: InvalidAgx,
    },
    /// Основа ACES с такими ручками не строится (при указанной остаточной подтяжке AE).
    #[error("viewfinder.tone (основа ACES): {source} (при остаточной подтяжке AE {residual_ev:+.2} EV)")]
    AcesUnusable {
        /// При какой подтяжке AE основа не строится, EV.
        residual_ev: f32,
        /// Что именно не так с основой.
        #[source]
        source: InvalidAces,
    },
    /// Основа filmic с такими ручками не строится (при указанной остаточной подтяжке AE).
    #[error("viewfinder.tone (основа filmic): {source} (при остаточной подтяжке AE {residual_ev:+.2} EV)")]
    FilmicUnusable {
        /// При какой подтяжке AE основа не строится, EV.
        residual_ev: f32,
        /// Что именно не так с основой.
        #[source]
        source: InvalidFilmic,
    },
    /// В секции модели задан свой средний серый сцены: это цель AE, а не ручка модели.
    #[error(
        "viewfinder.tone.{section}.scene_middle_gray = {value}: это цель автоэкспозиции, она не настраивается \
         (нужно {ae_target})"
    )]
    SceneMiddleGrayIsAeTarget {
        /// Секция модели (`agx`, `aces`, `filmic`).
        section: &'static str,
        /// Заданное значение.
        value: f32,
        /// Цель AE, которой обязаны равняться все модели.
        ae_target: f32,
    },
    /// Предел подтяжки AE не переводится в конечный множитель (проверить тон на краях нельзя).
    #[error("viewfinder.tone: предел подтяжки AE {max_residual_ev} EV не даёт конечного множителя")]
    ResidualLimitUnusable {
        /// Предел из `viewfinder.auto_exposure.controller.max_residual_ev`.
        max_residual_ev: f32,
    },
}

impl ViewfinderToneConfig {
    /// Полные настройки кривой: ручки секции поверх умолчания `isp-model`. Средний серый сцены, белый и
    /// сохранение оттенка всегда из умолчания (см. модуль: средний серый сцены — цель AE, не ручка).
    pub fn curve_settings(&self) -> ToneCurveSettings {
        ToneCurveSettings {
            contrast: self.contrast,
            display_middle_gray: self.display_middle_gray,
            ..ToneCurveSettings::default()
        }
    }

    /// Во сколько раз ручка контраста отличается от умолчания; ровно 1, когда ручка не тронута (тогда наклон
    /// модели умножается на 1 и остаётся бит в бит).
    fn contrast_ratio(&self) -> f32 {
        self.contrast / ToneCurveSettings::default().contrast
    }

    /// Серый экрана модели: ручка перекрывает поле секции, если её тронули; иначе — значение секции.
    fn display_gray_over(&self, section_value: f32) -> f32 {
        if self.display_middle_gray == ToneCurveSettings::default().display_middle_gray {
            section_value
        } else {
            self.display_middle_gray
        }
    }

    /// Настройки AgX: секция `agx` с применёнными ручками (правило — в доке модуля).
    pub fn agx_settings(&self) -> AgxSettings {
        AgxSettings {
            slope: self.agx.slope * self.contrast_ratio(),
            display_middle_gray: self.display_gray_over(self.agx.display_middle_gray),
            ..self.agx
        }
    }

    /// Настройки ACES: секция `aces` с применёнными ручками (правило — в доке модуля).
    pub fn aces_settings(&self) -> AcesSettings {
        AcesSettings {
            contrast: self.aces.contrast * self.contrast_ratio(),
            display_middle_gray: self.display_gray_over(self.aces.display_middle_gray),
            ..self.aces
        }
    }

    /// Настройки filmic: секция `filmic` с применёнными ручками (правило — в доке модуля).
    pub fn filmic_settings(&self) -> FilmicSettings {
        FilmicSettings {
            slope: self.filmic.slope * self.contrast_ratio(),
            display_middle_gray: self.display_gray_over(self.filmic.display_middle_gray),
            ..self.filmic
        }
    }

    /// Основа тона, которую выбрал конфиг (`base`): стартовая основа видоискателя и снимка, пока библиотека видов
    /// не сообщила свою. Не дублирует разбор моделей: берёт [`Self::tone_base_for`].
    pub fn tone_base(&self) -> ToneBase {
        self.tone_base_for(self.base.into())
    }

    /// Основа тона для модели `model` с её секцией настроек из этого конфига: ОДНА точка правды для видоискателя
    /// и снимка (решение 195: основа — часть вида и меняется на ходу, поэтому разрешается по запросу, а не только
    /// по `base`). Секции всех моделей лежат здесь же, поэтому смена модели не требует нового конфига.
    pub fn tone_base_for(&self, model: ToneBaseModel) -> ToneBase {
        match model {
            ToneBaseModel::Agx => ToneBase::Agx(self.agx_settings()),
            ToneBaseModel::Aces => ToneBase::Aces(self.aces_settings()),
            ToneBaseModel::Filmic => ToneBase::Filmic(self.filmic_settings()),
            ToneBaseModel::Legacy => ToneBase::Legacy,
        }
    }

    /// Проверяет, что ВСЕ модели строятся без подтяжки и на обоих краях `±max_residual_ev` (см. модуль).
    /// Строимость монотонна по подтяжке, поэтому краёв достаточно; основы проверяются для обоих охватов экрана.
    pub fn validate(&self, max_residual_ev: f32) -> Result<(), ToneConfigError> {
        self.validate_scene_middle_gray_is_the_ae_target()?;
        self.validate_legacy_curve(max_residual_ev)?;
        // Порядок — как в `ToneBaseChoice`; смена выбранной модели не должна открывать новую ошибку запуска.
        self.validate_working_base(&ToneBase::Agx(self.agx_settings()), max_residual_ev)?;
        self.validate_working_base(&ToneBase::Aces(self.aces_settings()), max_residual_ev)?;
        self.validate_working_base(&ToneBase::Filmic(self.filmic_settings()), max_residual_ev)
    }

    /// Средний серый сцены во всех секциях моделей равен цели AE (она не ручка).
    fn validate_scene_middle_gray_is_the_ae_target(&self) -> Result<(), ToneConfigError> {
        let ae_target = ToneCurveSettings::default().scene_middle_gray;
        for (section, value) in [
            ("agx", self.agx.scene_middle_gray),
            ("aces", self.aces.scene_middle_gray),
            ("filmic", self.filmic.scene_middle_gray),
        ] {
            if value != ae_target {
                return Err(ToneConfigError::SceneMiddleGrayIsAeTarget {
                    section,
                    value,
                    ae_target,
                });
            }
        }
        Ok(())
    }

    /// Прежняя кривая строится на краях подтяжки.
    fn validate_legacy_curve(&self, max_residual_ev: f32) -> Result<(), ToneConfigError> {
        let settings = self.curve_settings();
        for gain_ev in residual_edges(max_residual_ev) {
            let (residual_ev, gain) = exposure_gain_at(gain_ev, max_residual_ev)?;
            ToneCurve::resolve(&settings.with_exposure_gain(gain))
                .map_err(|source| ToneConfigError::CurveUnusable { residual_ev, source })?;
        }
        Ok(())
    }

    /// Основа в рабочем пространстве строится на краях подтяжки для обоих охватов экрана.
    fn validate_working_base(&self, base: &ToneBase, max_residual_ev: f32) -> Result<(), ToneConfigError> {
        for gain_ev in residual_edges(max_residual_ev) {
            let (residual_ev, gain) = exposure_gain_at(gain_ev, max_residual_ev)?;
            for gamut in [DisplayGamut::Srgb, DisplayGamut::DisplayP3] {
                base.resolve(gain, gamut).map_err(|error| match error {
                    InvalidToneBase::Agx(source) => ToneConfigError::AgxUnusable { residual_ev, source },
                    InvalidToneBase::Aces(source) => ToneConfigError::AcesUnusable { residual_ev, source },
                    InvalidToneBase::Filmic(source) => ToneConfigError::FilmicUnusable { residual_ev, source },
                })?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, reason = "тест")]

    use isp_model::tone::knobs::ToneModelKind;

    use super::*;

    /// Предел подтяжки AE по умолчанию (`ExposureControllerConfig::max_residual_ev`).
    const DEFAULT_RESIDUAL_EV: f32 = 0.5;

    #[test]
    fn default_matches_the_isp_model_default_and_is_valid() {
        let config = ViewfinderToneConfig::default();
        assert_eq!(config.curve_settings(), ToneCurveSettings::default());
        assert_eq!(config.validate(DEFAULT_RESIDUAL_EV), Ok(()));
    }

    #[test]
    fn knobs_never_move_the_scene_middle_gray_that_auto_exposure_aims_at() {
        let config = ViewfinderToneConfig {
            contrast: 1.8,
            display_middle_gray: 0.24,
            ..ViewfinderToneConfig::default()
        };
        let settings = config.curve_settings();
        let model = ToneCurveSettings::default();
        assert_eq!(settings.contrast, 1.8);
        assert_eq!(settings.display_middle_gray, 0.24);
        assert_eq!(settings.scene_middle_gray, model.scene_middle_gray, "цель AE не ручка");
        assert_eq!(settings.scene_white, model.scene_white);
        assert_eq!(settings.hue_preservation, model.hue_preservation);
    }

    #[test]
    fn json_fills_missing_fields_from_defaults_and_rejects_typos() {
        let partial: ViewfinderToneConfig = serde_json::from_str(r#"{"contrast": 1.5}"#).unwrap();
        assert_eq!(partial.contrast, 1.5);
        assert_eq!(
            partial.display_middle_gray,
            ViewfinderToneConfig::default().display_middle_gray
        );
        assert!(serde_json::from_str::<ViewfinderToneConfig>(r#"{"contrats": 1.5}"#).is_err());
        assert!(
            serde_json::from_str::<ViewfinderToneConfig>(r#"{"scene_middle_gray": 0.1}"#).is_err(),
            "цель AE через конфиг тона не задаётся"
        );
    }

    #[test]
    fn non_positive_contrast_is_rejected_with_the_model_reason() {
        let config = ViewfinderToneConfig {
            contrast: 0.0,
            ..ViewfinderToneConfig::default()
        };
        assert_eq!(
            config.validate(DEFAULT_RESIDUAL_EV),
            Err(ToneConfigError::CurveUnusable {
                residual_ev: 0.0,
                source: InvalidToneCurve::InvalidContrast { contrast: 0.0 },
            })
        );
    }

    #[test]
    fn display_middle_gray_outside_zero_one_is_rejected() {
        let config = ViewfinderToneConfig {
            display_middle_gray: 1.0,
            ..ViewfinderToneConfig::default()
        };
        assert!(matches!(
            config.validate(DEFAULT_RESIDUAL_EV),
            Err(ToneConfigError::CurveUnusable {
                source: InvalidToneCurve::DisplayMiddleGrayOutOfRange { .. },
                ..
            })
        ));
    }

    /// Сочетание строится без подтяжки, но не при крайней затемняющей подтяжке AE: проверка обязана поймать
    /// его при старте (иначе ISP молча держал бы прежние параметры на тех кадрах).
    #[test]
    fn combination_that_breaks_only_at_the_darkening_residual_edge_is_rejected() {
        // Предел строимости: y_g > (g / w)^c. При g = 0,075, c = 0,5: без подтяжки 0,274, при k = 2^−0,5 — 0,326.
        let config = ViewfinderToneConfig {
            contrast: 0.5,
            display_middle_gray: 0.3,
            ..ViewfinderToneConfig::default()
        };
        assert_eq!(
            config.validate_legacy_curve(0.0),
            Ok(()),
            "без подтяжки строится — значит, ловит именно край"
        );
        assert!(matches!(
            config.validate_legacy_curve(DEFAULT_RESIDUAL_EV),
            Err(ToneConfigError::CurveUnusable {
                residual_ev,
                source: InvalidToneCurve::DisplayMiddleGrayUnreachable { .. },
            }) if residual_ev < 0.0
        ));
    }

    #[test]
    fn residual_limit_without_a_finite_multiplier_is_its_own_error() {
        assert_eq!(
            ViewfinderToneConfig::default().validate(1.0e6),
            Err(ToneConfigError::ResidualLimitUnusable { max_residual_ev: 1.0e6 })
        );
    }

    /// AgX строится без подтяжки, но не на затемняющем краю: меньший запас до белого требует большего наклона.
    /// Проверка при старте обязана поймать это и назвать основу.
    #[test]
    fn agx_slope_that_breaks_only_at_the_darkening_residual_edge_is_rejected() {
        // Наклон 0,18 · 1,05 / 1,35 = 0,14: больше порога без подтяжки (0,131), меньше порога при k = 2^-0,5 (0,151).
        let config = ViewfinderToneConfig {
            contrast: 1.05,
            ..ViewfinderToneConfig::default()
        };
        assert_eq!(config.validate(0.0), Ok(()), "без подтяжки строится обе основы");
        assert!(matches!(
            config.validate(DEFAULT_RESIDUAL_EV),
            Err(ToneConfigError::AgxUnusable { residual_ev, .. }) if residual_ev < 0.0
        ));
    }

    /// Обе основы проверяются независимо от выбранной: «прежняя» с контрастом, который ломает AgX, всё равно ошибка
    /// (выбор основы можно сменить без правки остального конфига).
    #[test]
    fn both_bases_are_validated_whichever_one_is_selected() {
        let config = ViewfinderToneConfig {
            base: ToneBaseChoice::Legacy,
            contrast: 1.05,
            ..ViewfinderToneConfig::default()
        };
        assert!(matches!(
            config.validate(DEFAULT_RESIDUAL_EV),
            Err(ToneConfigError::AgxUnusable { .. })
        ));
    }

    /// Основа любой модели разрешается из секций конфига независимо от выбранной `base`, а `tone_base()` — это
    /// `tone_base_for(base)`: одна точка правды.
    #[test]
    fn tone_base_for_resolves_every_model_from_its_own_section_whichever_base_is_selected() {
        let config = ViewfinderToneConfig {
            base: ToneBaseChoice::Legacy,
            aces: AcesSettings {
                glow_gain: 0.0,
                ..AcesSettings::default()
            },
            ..ViewfinderToneConfig::default()
        };
        for (model, kind) in [
            (ToneBaseModel::Agx, ToneModelKind::Agx),
            (ToneBaseModel::Aces, ToneModelKind::Aces),
            (ToneBaseModel::Filmic, ToneModelKind::Filmic),
            (ToneBaseModel::Legacy, ToneModelKind::Legacy),
        ] {
            assert_eq!(config.tone_base_for(model).kind(), kind, "{model:?}");
        }
        let ToneBase::Aces(aces) = config.tone_base_for(ToneBaseModel::Aces) else {
            panic!("ожидалась ACES");
        };
        assert_eq!(aces, config.aces_settings(), "секция ACES конфига");
        assert_eq!(
            config.tone_base(),
            ToneBase::Legacy,
            "выбранная base — стартовая модель"
        );
    }

    #[test]
    fn base_choice_selects_the_tone_base_and_defaults_to_agx() {
        let config = ViewfinderToneConfig::default();
        assert_eq!(config.base, ToneBaseChoice::Agx);
        assert!(matches!(config.tone_base(), ToneBase::Agx(_)));
        let legacy: ViewfinderToneConfig = serde_json::from_str(r#"{"base": "legacy"}"#).unwrap();
        assert_eq!(legacy.tone_base(), ToneBase::Legacy);
        let agx: ViewfinderToneConfig = serde_json::from_str(r#"{"base": "agx"}"#).unwrap();
        assert!(matches!(agx.tone_base(), ToneBase::Agx(_)));
        assert!(serde_json::from_str::<ViewfinderToneConfig>(r#"{"base": "reinhard"}"#).is_err());
    }

    /// Каждое имя модели выбирает свою основу, и настройки модели берутся из её секции с общими ручками.
    #[test]
    fn every_base_name_selects_its_own_model_with_its_section_settings() {
        for (name, kind) in [
            ("agx", ToneModelKind::Agx),
            ("aces", ToneModelKind::Aces),
            ("filmic", ToneModelKind::Filmic),
            ("legacy", ToneModelKind::Legacy),
        ] {
            let config: ViewfinderToneConfig = serde_json::from_str(&format!(r#"{{"base": "{name}"}}"#)).unwrap();
            assert_eq!(config.tone_base().kind(), kind, "{name}");
            assert_eq!(config.base.model_kind(), kind, "{name}");
            assert_eq!(
                config.validate(DEFAULT_RESIDUAL_EV),
                Ok(()),
                "{name}: умолчания строятся"
            );
        }
        // Секции моделей приходят в основу как есть (ручки не тронуты): так владелец крутит их через конфиг.
        let config: ViewfinderToneConfig =
            serde_json::from_str(r#"{"base": "aces", "aces": {"glow_gain": 0.0}, "filmic": {"slope": 0.2}}"#).unwrap();
        let ToneBase::Aces(aces) = config.tone_base() else {
            panic!("выбрана ACES");
        };
        assert_eq!(aces.glow_gain, 0.0);
        assert_eq!(
            aces,
            AcesSettings {
                glow_gain: 0.0,
                ..AcesSettings::default()
            }
        );
        assert_eq!(
            config.filmic_settings().slope,
            0.2,
            "секция невыбранной модели тоже разбирается"
        );
    }

    /// Ручки в умолчании модель не трогают бит в бит; тронутые — одним правилом для каждой модели.
    #[test]
    fn knobs_follow_one_rule_for_every_model() {
        let default_contrast = ToneCurveSettings::default().contrast;
        let untouched = ViewfinderToneConfig::default();
        assert_eq!(untouched.agx_settings(), AgxSettings::default());
        assert_eq!(untouched.aces_settings(), AcesSettings::default());
        assert_eq!(untouched.filmic_settings(), FilmicSettings::default());

        let tuned = ViewfinderToneConfig {
            contrast: 2.0 * default_contrast,
            display_middle_gray: 0.24,
            ..ViewfinderToneConfig::default()
        };
        assert_eq!(tuned.agx_settings().slope, 2.0 * AgxSettings::default().slope);
        assert_eq!(tuned.filmic_settings().slope, 2.0 * FilmicSettings::default().slope);
        assert_eq!(tuned.aces_settings().contrast, 2.0 * AcesSettings::default().contrast);
        for gray in [
            tuned.agx_settings().display_middle_gray,
            tuned.aces_settings().display_middle_gray,
            tuned.filmic_settings().display_middle_gray,
        ] {
            assert_eq!(gray, 0.24);
        }
        // Тронутая ручка масштабирует то, что владелец задал в секции, а не затирает его умолчанием.
        let customised = ViewfinderToneConfig {
            contrast: 2.0 * default_contrast,
            aces: AcesSettings {
                contrast: 1.1,
                ..AcesSettings::default()
            },
            ..ViewfinderToneConfig::default()
        };
        assert_eq!(customised.aces_settings().contrast, 2.2);
        // Серый экрана из секции сохраняется, пока ручка серого в умолчании.
        let section_gray = ViewfinderToneConfig {
            filmic: FilmicSettings {
                display_middle_gray: 0.18,
                ..FilmicSettings::default()
            },
            ..ViewfinderToneConfig::default()
        };
        assert_eq!(section_gray.filmic_settings().display_middle_gray, 0.18);
    }

    /// Смена `base` не должна неожиданно ронять запуск: сломанная секция ЛЮБОЙ модели отвергается, даже если
    /// выбрана другая, и ошибка называет модель.
    #[test]
    fn every_model_section_is_validated_whichever_base_is_selected() {
        let broken_aces = ViewfinderToneConfig {
            aces: AcesSettings {
                shape_width_ev: 0.0,
                ..AcesSettings::default()
            },
            ..ViewfinderToneConfig::default()
        };
        assert!(matches!(
            broken_aces.validate(DEFAULT_RESIDUAL_EV),
            Err(ToneConfigError::AcesUnusable { .. })
        ));
        let broken_filmic = ViewfinderToneConfig {
            filmic: FilmicSettings {
                slope: 0.05,
                ..FilmicSettings::default()
            },
            ..ViewfinderToneConfig::default()
        };
        assert!(matches!(
            broken_filmic.validate(DEFAULT_RESIDUAL_EV),
            Err(ToneConfigError::FilmicUnusable { .. })
        ));
        // Край подтяжки AE: наклон строится без неё и ломается на затемняющем краю (как у AgX).
        let edge_only = ViewfinderToneConfig {
            filmic: FilmicSettings {
                slope: 0.145,
                ..FilmicSettings::default()
            },
            ..ViewfinderToneConfig::default()
        };
        assert_eq!(edge_only.validate(0.0), Ok(()), "без подтяжки строится");
        assert!(matches!(
            edge_only.validate(DEFAULT_RESIDUAL_EV),
            Err(ToneConfigError::FilmicUnusable { residual_ev, .. }) if residual_ev < 0.0
        ));
    }

    /// Цель AE одна для всех моделей: секция не может сдвинуть её молча.
    #[test]
    fn a_model_section_cannot_move_the_ae_target() {
        let moved = ViewfinderToneConfig {
            aces: AcesSettings {
                scene_middle_gray: 0.1,
                ..AcesSettings::default()
            },
            ..ViewfinderToneConfig::default()
        };
        assert_eq!(
            moved.validate(DEFAULT_RESIDUAL_EV),
            Err(ToneConfigError::SceneMiddleGrayIsAeTarget {
                section: "aces",
                value: 0.1,
                ae_target: ToneCurveSettings::default().scene_middle_gray,
            })
        );
    }

    /// Секции моделей разбираются из JSON; опечатка в поле секции и чужое поле отвергаются.
    #[test]
    fn model_sections_round_trip_through_json_and_reject_unknown_fields() {
        let config = ViewfinderToneConfig {
            base: ToneBaseChoice::Filmic,
            ..ViewfinderToneConfig::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        assert_eq!(serde_json::from_str::<ViewfinderToneConfig>(&json).unwrap(), config);
        assert!(serde_json::from_str::<ViewfinderToneConfig>(r#"{"aces": {"glow": 0.1}}"#).is_err());
        assert!(serde_json::from_str::<ViewfinderToneConfig>(r#"{"reinhard": {}}"#).is_err());
    }

    /// Выбор любой модели основы проходит проверку запуска видоискателя: все четыре считаются на GPU, отказа
    /// «модель не поддержана» нет (его снимали пакеты GPU-моделей по одной).
    #[test]
    fn selecting_any_tone_base_passes_the_viewfinder_startup_validation() {
        for base in [
            ToneBaseChoice::Agx,
            ToneBaseChoice::Legacy,
            ToneBaseChoice::Aces,
            ToneBaseChoice::Filmic,
        ] {
            let config = crate::config::ViewfinderConfig {
                tone: ViewfinderToneConfig {
                    base,
                    ..ViewfinderToneConfig::default()
                },
                ..crate::config::ViewfinderConfig::default()
            };
            assert_eq!(config.validate(), Ok(()), "{base:?}");
        }
    }

    /// Ручки применяются к AgX одним правилом: серый экрана — как есть, наклон — пропорционально контрасту;
    /// контраст по умолчанию даёт ровно умолчание AgX, а средний серый сцены (цель AE) не двигается.
    #[test]
    fn knobs_map_onto_the_agx_base_proportionally_and_keep_the_ae_target() {
        let model = AgxSettings::default();
        let default_settings = ViewfinderToneConfig::default().agx_settings();
        assert_eq!(default_settings, model, "умолчание ручек = умолчание AgX");
        let tuned = ViewfinderToneConfig {
            contrast: 2.0 * ToneCurveSettings::default().contrast,
            display_middle_gray: 0.24,
            ..ViewfinderToneConfig::default()
        }
        .agx_settings();
        assert_eq!(tuned.slope, 2.0 * model.slope);
        assert_eq!(tuned.display_middle_gray, 0.24);
        assert_eq!(tuned.scene_middle_gray, model.scene_middle_gray, "цель AE не ручка");
        assert_eq!(
            model.scene_middle_gray,
            ToneCurveSettings::default().scene_middle_gray,
            "у обеих основ одна цель AE"
        );
    }
}
