//! Факты о виде для экрана калибровки (14b): приложение собирает [`LookFacts`] и отдаёт экрану, ТОЛЬКО когда
//! они изменились.
//!
//! Экран не видит ни библиотеку пресетов, ни конфиг модели вида, поэтому всё, что он показывает (список пресетов,
//! какие ручки отличаются от пресета, цвета точек полос, ручки, которых основа не умеет, плашка проблемы), приходит
//! одним сравниваемым значением. Шов — тот же, что у фактов камеры (`camera_screen_facts`): источник собирает
//! снимок, наблюдатель сравнивает с последним ОТПРАВЛЕННЫМ по `==` и отдаёт новый, только если он другой.
//!
//! # Откуда что берётся
//! * активный пресет, рецепты, правки, список пресетов — библиотека пресетов ([`LookHandle::view_state`]);
//! * проблема — исполнитель вида ([`crate::look_effects::LookEffects::latest_problem`]); номер проблемы растёт,
//!   поэтому две одинаковые подряд различимы и экран покажет обе;
//! * полосы и цвета их точек — каталог из конфига рендера ([`LookFactsCatalog`]): идентификаторы и центры
//!   оттенка модели вида. Цвет точки считается ЗДЕСЬ, экран его только рисует;
//! * ручки, которых текущая основа не умеет, — ПРОБА самой модели (`isp-model`, шов ручек тона): каждой ручке тона
//!   даётся ненулевое значение, и всё, на что модель отвечает «не поддерживаю», попадает в список. Ничего не
//!   зашито вручную («у legacy нет теней»): новая модель или новая ручка проявятся сами.
//!
//! # Без библиотеки
//! Сборка без хранилища пресетов (`LookHandle` нет): источника фактов не существует, экран фактов НЕ получает и
//! остаётся с ручками отключённо (`ScreenState::look_facts() == None`, это явное состояние экрана, а не ошибка).

use isp_model::color::chromaticity::Primaries;
use isp_model::color::oklab_working::OklabWorkspace;
use isp_model::tone::ToneCurveSettings;
use isp_model::tone::knobs::{ToneKnob, ToneKnobAmounts};
use isp_model::transfer::srgb_encode;
use look_contract::{BandFacts, BandSwatch, GlobalKnob, LookFacts, LookProblem, ToneBaseModel};
use render_loop::config::{RenderLoopConfig, ViewfinderToneConfig};
use serde::{Deserialize, Serialize};

use crate::look_service::LookHandle;

const LOG_TARGET: &str = "fasticamera::look";

/// Как выглядит цветная точка полосы на экране калибровки (решение 200): светлота и цветность OKLCh, оттенок —
/// центр полосы из конфига рендера. Секция `camera_screen.band_swatch`; числа — `[estimate]`, подбор глазами в 14j.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct BandSwatchStyle {
    /// Светлота OKLab L (0…1]. `[estimate]` 0,72: средняя светлота, при которой все оттенки круга ещё
    /// укладываются в sRGB при цветности ниже.
    pub lightness: f32,
    /// Цветность OKLCh C [0…`MAX_SWATCH_CHROMA`]. `[estimate]` 0,12: полосы отличаются друг от друга, а синий и
    /// пурпурный не вылетают за охват sRGB (вылет всё равно обрезается при кодировании).
    pub chroma: f32,
}

/// Верхняя граница цветности точки: дальше любой оттенок далеко за охватом sRGB, и точки сливаются в обрезанные
/// чистые цвета экрана.
pub const MAX_SWATCH_CHROMA: f32 = 0.4;

impl Default for BandSwatchStyle {
    fn default() -> Self {
        Self {
            lightness: 0.72,
            chroma: 0.12,
        }
    }
}

/// Почему секция `camera_screen.band_swatch` неверна.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum BandSwatchStyleError {
    #[error("camera_screen.band_swatch.lightness = {value}: нужно больше 0 и не больше 1")]
    Lightness { value: f32 },
    #[error("camera_screen.band_swatch.chroma = {value}: нужно от 0 до {MAX_SWATCH_CHROMA}")]
    Chroma { value: f32 },
}

impl BandSwatchStyle {
    /// Проверка при разборе конфига: NaN и значения вне диапазона отвергаются целиком, а не обрезаются молча.
    pub fn validate(&self) -> Result<(), BandSwatchStyleError> {
        if !(self.lightness > 0.0 && self.lightness <= 1.0) {
            return Err(BandSwatchStyleError::Lightness { value: self.lightness });
        }
        if !(self.chroma >= 0.0 && self.chroma <= MAX_SWATCH_CHROMA) {
            return Err(BandSwatchStyleError::Chroma { value: self.chroma });
        }
        Ok(())
    }
}

/// Величина пробной ручки тона: любое ненулевое значение (нейтральные ручки модель не опрашивает).
const PROBE_KNOB_AMOUNT: f32 = 1.0;

/// Что известно о модели вида из конфига рендера: полосы с цветами точек и настройки основ для пробы ручек.
#[derive(Debug, Clone)]
pub struct LookFactsCatalog {
    bands: Vec<BandFacts>,
    /// Ручки, которых не умеет каждая основа. Проба дорогая (строит основу и гоняет ручки), а ответ зависит только
    /// от конфига, поэтому считается один раз при сборке каталога, а не на каждый кадр.
    unsupported_by_base: Vec<(ToneBaseModel, Vec<GlobalKnob>)>,
}

impl LookFactsCatalog {
    /// Каталог по конфигу рендера: те же полосы и те же секции основ, по которым работает видоискатель; вид точек —
    /// из конфига экрана (`swatch`).
    pub fn from_render_config(config: &RenderLoopConfig, swatch: &BandSwatchStyle) -> Self {
        let workspace = OklabWorkspace::new(&Primaries::SRGB);
        let bands = config
            .viewfinder
            .look_model
            .bands
            .iter()
            .map(|band| BandFacts {
                id: band.id.clone(),
                swatch: swatch_for_hue(workspace.as_ref().ok(), swatch, band.center_degrees),
            })
            .collect();
        let unsupported_by_base = ToneBaseModel::ALL
            .into_iter()
            .map(|base| (base, probe_unsupported_knobs(&config.viewfinder.tone, base)))
            .collect();
        Self {
            bands,
            unsupported_by_base,
        }
    }

    /// Полосы модели вида с цветами точек (порядок конфига).
    pub fn bands(&self) -> &[BandFacts] {
        &self.bands
    }

    /// Ручки, которых основа `base` не умеет (из кэша, посчитанного при сборке каталога пробой — см.
    /// [`probe_unsupported_knobs`]); вид и порядок — как у [`GlobalKnob::ALL`].
    pub fn unsupported_knobs(&self, base: ToneBaseModel) -> Vec<GlobalKnob> {
        self.unsupported_by_base
            .iter()
            .find(|(known, _)| *known == base)
            .map(|(_, knobs)| knobs.clone())
            .unwrap_or_default()
    }
}

/// Проба: какие ручки основа `base` не умеет. Идёт через тот же шов ручек тона, которым пользуется рендер
/// (`ToneBase::with_knobs`), на настройках основы из конфига.
fn probe_unsupported_knobs(tone: &ViewfinderToneConfig, base: ToneBaseModel) -> Vec<GlobalKnob> {
    let tone_base = tone.tone_base_for(base);
    let legacy = ToneCurveSettings::default();
    GlobalKnob::ALL
        .into_iter()
        .filter(|knob| {
            let Some(tone_knob) = tone_knob_of(*knob) else {
                // Цветовые ручки (насыщенность, сочность, теплота, оттенок) от основы не зависят.
                return false;
            };
            let probe = single_knob_amounts(tone_knob);
            tone_base
                .with_knobs(&legacy, &probe)
                .unsupported
                .iter()
                .any(|refusal| refusal.knob == tone_knob)
        })
        .collect()
}

/// Какая ручка тона стоит за глобальной ручкой вида; `None` — ручка не про тон.
fn tone_knob_of(knob: GlobalKnob) -> Option<ToneKnob> {
    match knob {
        GlobalKnob::Contrast => Some(ToneKnob::Contrast),
        GlobalKnob::Midtones => Some(ToneKnob::Midtones),
        GlobalKnob::Shadows => Some(ToneKnob::Shadows),
        GlobalKnob::Highlights => Some(ToneKnob::Highlights),
        GlobalKnob::Saturation | GlobalKnob::Vibrance | GlobalKnob::Warmth | GlobalKnob::Tint => None,
    }
}

/// Набор значений, где ненулевая только `knob`.
fn single_knob_amounts(knob: ToneKnob) -> ToneKnobAmounts {
    let mut amounts = ToneKnobAmounts::default();
    match knob {
        ToneKnob::Contrast => amounts.contrast_log2 = PROBE_KNOB_AMOUNT,
        ToneKnob::Midtones => amounts.midtones_ev = PROBE_KNOB_AMOUNT,
        ToneKnob::Shadows => amounts.shadows = PROBE_KNOB_AMOUNT,
        ToneKnob::Highlights => amounts.highlights = PROBE_KNOB_AMOUNT,
    }
    amounts
}

/// Цвет точки полосы: центр оттенка при фиксированных светлоте и цветности → sRGB 8 бит (вне охвата обрезается).
/// Матрицы OKLab для sRGB не строятся (вырожденные основные цвета) — серая точка и громкая запись в лог.
fn swatch_for_hue(workspace: Option<&OklabWorkspace>, style: &BandSwatchStyle, hue_degrees: f32) -> BandSwatch {
    const NEUTRAL_SWATCH: BandSwatch = BandSwatch { srgb: [128, 128, 128] };
    let Some(workspace) = workspace else {
        tracing::error!(target: LOG_TARGET, "матрицы OKLab для sRGB не построены: точки полос серые");
        return NEUTRAL_SWATCH;
    };
    let hue_radians = hue_degrees.to_radians();
    let lab = [
        style.lightness,
        style.chroma * hue_radians.cos(),
        style.chroma * hue_radians.sin(),
    ];
    let linear = workspace.to_rgb(lab);
    BandSwatch {
        // `srgb_encode` обрезает вход в 0…1; значение 0…1 → 0…255 с округлением.
        srgb: linear.map(|channel| (srgb_encode(channel) * 255.0).round() as u8),
    }
}

/// Итог наблюдения: факты изменились или остались прежними.
#[derive(Debug, Clone, PartialEq)]
pub enum LookFactsChange {
    /// Факты отличаются от последних отправленных: их надо отдать экрану.
    Changed(Box<LookFacts>),
    /// Те же факты: экрану слать нечего.
    Unchanged,
}

/// Источник фактов о виде и наблюдатель за их изменением (живёт в источнике UI экрана, на потоке рендера).
pub struct LookFactsSource {
    handle: LookHandle,
    catalog: LookFactsCatalog,
    /// Последние факты, ОТДАННЫЕ экрану; `None` — ещё ничего не отдавали (первый снимок всегда «изменился»).
    last_sent: Option<LookFacts>,
    /// От чего собраны последние факты: пока версия библиотеки и проблема те же, пересобирать нечего.
    last_inputs: Option<FactsInputs>,
    /// Сколько раз факты пересобирались (диагностика: тихие кадры не должны его двигать).
    rebuilds: u64,
}

/// Всё, от чего зависят факты, кроме неизменного каталога: версия библиотеки и проблема вида.
#[derive(Debug, Clone, PartialEq)]
struct FactsInputs {
    library_revision: u64,
    problem: Option<LookProblem>,
}

impl LookFactsSource {
    /// Источник поверх ручки на библиотеку и каталога модели вида.
    pub fn new(handle: LookHandle, catalog: LookFactsCatalog) -> Self {
        Self {
            handle,
            catalog,
            last_sent: None,
            last_inputs: None,
            rebuilds: 0,
        }
    }

    /// Сколько раз факты пересобирались с начала жизни источника.
    pub fn rebuilds(&self) -> u64 {
        self.rebuilds
    }

    /// Текущие факты: снимок библиотеки, каталог полос, ручки текущей основы и проблема (её даёт исполнитель вида).
    pub fn current_facts(&self, problem: Option<LookProblem>) -> LookFacts {
        let state = self.handle.view_state();
        let current = state.active.recipe;
        LookFacts {
            active: state.active.preset,
            presets: state.presets,
            unsupported_knobs: self.catalog.unsupported_knobs(current.base),
            current,
            preset: state.active.preset_recipe,
            edits: state.active.edits,
            bands: self.catalog.bands().to_vec(),
            problem,
        }
    }

    /// Сверяет текущие факты с последними отправленными: изменившиеся запоминает как отправленные и отдаёт.
    ///
    /// Зовётся на каждом кадре, поэтому дёшево: пока версия библиотеки и проблема не менялись, факты не
    /// пересобираются (ни замка со снимком, ни копий пресетов).
    pub fn observe(&mut self, problem: Option<LookProblem>) -> LookFactsChange {
        let inputs = FactsInputs {
            library_revision: self.handle.revision(),
            problem,
        };
        if self.last_inputs.as_ref() == Some(&inputs) {
            return LookFactsChange::Unchanged;
        }
        let facts = self.current_facts(inputs.problem.clone());
        self.rebuilds += 1;
        self.last_inputs = Some(inputs);
        if self.last_sent.as_ref() == Some(&facts) {
            return LookFactsChange::Unchanged;
        }
        self.last_sent = Some(facts.clone());
        LookFactsChange::Changed(Box::new(facts))
    }
}

#[cfg(test)]
#[path = "look_facts_tests.rs"]
mod tests;
