//! Выбор основы тона: прежняя кривая (вариант сравнения до 14b) или AgX. Основа — сменная МОДЕЛЬ (решение 182).
//!
//! # Как добавить модель основы (ACES, filmic и т. д.)
//! Слой вида (ручки тона и цвета, LUT, сдвиг белой) работает над светом экрана ПОСЛЕ основы и о модели ничего не
//! знает. Поэтому новая модель добавляется без правки модуля вида и сразу получает все ручки (решения 236, 244).
//!
//! **CPU (`isp-model`):**
//! 1. Свой тип настроек (`serde`, `deny_unknown_fields`, `default`, проверка с enum-ошибкой, числа `[estimate]`).
//! 2. Вариант в [`ToneBase`] и [`ResolvedToneBase`] (разрешённая основа с числами в f32), ветки в
//!    [`ToneBase::working_primaries`] (рабочее пространство `W` модели) и
//!    [`ResolvedToneBase::white_policy`], в `IspParams::resolve` и в `pipeline::run_reference_*`.
//! 3. Контракт [`super::working_stage::WorkingToneBase`] для разрешённой основы (формулы пикселя) и ветка в
//!    [`ResolvedToneBase::working_stage`]: стадии над изображениями и эталонная цепочка у всех моделей ОДНИ.
//!    Вход в `W` и выход `W → цвета выхода` модель берёт у общего [`super::working_io::WorkingSpaceIo`], а не пишет сама.
//!
//! **GPU (`isp-vulkan`, решение 182):** ОДИН общий uniform-блок `ToneBaseParameters` под любую модель (массив
//! строк `vec4`; строка 0 общая, остальные — числа модели). Модель добавляет: свой Rust-упаковщик
//! `isp-vulkan/src/tone_base/<модель>.rs` (строки и их таблица), свой GLSL-файл `shaders/tone_base_<модель>.glsl`
//! (подключается общим include в видоискатель и в выходной проход снимка), номер модели (константа специализации
//! видоискателя `TONE_BASE_MODEL` и число в строке 0 для качественного пути и снимка). Полная инструкция — в доке
//! `isp-vulkan/src/tone_base/mod.rs` («Как добавить модель основы на GPU»).
//!
//! **Тесты, обязательные для модели:** серый сцены → ровно заданный серый экрана; белый точки → ровно 1;
//! серый остаётся серым на всём диапазоне; путь в белое без ступенек (R, G, B, C, M, Y); цель решения 47 (закат,
//! кожа, небо — потолки решения 181); цвета за охватом без NaN и выбросов; нейтральный вид — выход бит в бит;
//! кривая строится на краях подтяжки AE (`look::validation`); сила ручек тона на модели не ниже порога
//! (`tests/look_knob_strength.rs`); паритет CPU и GPU.

use std::sync::Arc;

use super::aces::{AcesBase, AcesSettings, InvalidAces};
use super::agx::AgxBase;
use super::agx_settings::{AgxSettings, InvalidAgx};
use super::filmic::{FilmicBase, FilmicSettings, InvalidFilmic};
use super::working_stage::WorkingToneBase;
use crate::color::chromaticity::Primaries;
use crate::color::working_space::WORKING_PRIMARIES;
use crate::exposure::ExposureGain;
use crate::image::DisplayGamut;

/// Какая основа тона формирует картинку (настройка обработки).
///
/// `Legacy` берёт числа из `ProcessingControls::tone`, остальные варианты несут настройки своей модели. Выбор живёт в
/// `ProcessingControls::tone_base` и разрешается один раз в `IspParams::resolve` (и для видоискателя, и для снимка).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToneBase {
    /// Лог-логистическая S-кривая с возвратом оттенка по HSV, сжатие ACES-RGC перед ней. Вариант сравнения
    /// до 14b: выбирается явно, умолчанием больше не является.
    Legacy,
    /// Основа AgX в рабочем пространстве Rec. 2020 (решения 101, 176, 177).
    Agx(AgxSettings),
    /// Основа ACES: RRT и выходное преобразование SDR 100 нит Academy (решения 176, 182).
    Aces(AcesSettings),
    /// Основа filmic: своя реализация идеи darktable filmic RGB (решения 176, 182).
    Filmic(FilmicSettings),
}

impl Default for ToneBase {
    /// Умолчание — AgX со стартовыми настройками (версия параметров 3, 14a).
    fn default() -> Self {
        Self::Agx(AgxSettings::default())
    }
}

/// Название модели основы (для сообщений, выбора на GPU и отчётов).
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

impl ToneBase {
    /// Какая это модель (для сообщений об отказе и выбора на GPU).
    pub fn kind(&self) -> ToneModelKind {
        match self {
            Self::Legacy => ToneModelKind::Legacy,
            Self::Agx(_) => ToneModelKind::Agx,
            Self::Aces(_) => ToneModelKind::Aces,
            Self::Filmic(_) => ToneModelKind::Filmic,
        }
    }

    /// Разрешает основу на кадр: настройки модели и остаточная подтяжка AE `gain` (решение 72) —
    /// серый кривой делится на `k`, белый точки остаётся прежним. Единственное место, где настройки модели
    /// становятся [`ResolvedToneBase`] (кадр и проверка краёв подтяжки зовут его же). У `Legacy` числа живут не в
    /// варианте, а в `ProcessingControls::tone`, поэтому разрешение — пустое.
    pub fn resolve(&self, gain: ExposureGain, output_gamut: DisplayGamut) -> Result<ResolvedToneBase, InvalidToneBase> {
        Ok(match self {
            Self::Legacy => ResolvedToneBase::Legacy,
            Self::Agx(settings) => {
                ResolvedToneBase::Agx(AgxBase::resolve(&settings.with_exposure_gain(gain), output_gamut)?)
            }
            Self::Aces(settings) => {
                ResolvedToneBase::Aces(AcesBase::resolve(&settings.with_exposure_gain(gain), output_gamut)?)
            }
            Self::Filmic(settings) => {
                ResolvedToneBase::Filmic(FilmicBase::resolve(&settings.with_exposure_gain(gain), output_gamut)?)
            }
        })
    }

    /// Рабочее пространство `W` модели: в нём считаются основа и слой вида. AgX, ACES и filmic — линейный
    /// Rec. 2020; прежняя основа работает в цветах выхода (`W = выход`, решение 176).
    pub fn working_primaries(&self, output_gamut: DisplayGamut) -> Primaries {
        match self {
            Self::Legacy => output_gamut.primaries(),
            Self::Agx(_) | Self::Aces(_) | Self::Filmic(_) => WORKING_PRIMARIES,
        }
    }
}

/// Основа тона не строится: ошибка модели, у каждой модели своя (различимы, текст — как у модели).
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum InvalidToneBase {
    #[error("основа AgX: {0}")]
    Agx(#[from] InvalidAgx),
    #[error("основа ACES: {0}")]
    Aces(#[from] InvalidAces),
    #[error("основа filmic: {0}")]
    Filmic(#[from] InvalidFilmic),
}

/// Основа тона, разрешённая на кадр.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedToneBase {
    /// Прежняя цепочка: числа лежат в `IspParams::tone` и `IspParams::gamut_compression`.
    Legacy,
    /// Основа AgX (общая `Arc`: клонирование параметров кадра дёшево).
    Agx(Arc<AgxBase>),
    /// Основа ACES.
    Aces(Arc<AcesBase>),
    /// Основа filmic.
    Filmic(Arc<FilmicBase>),
}

/// Политика обрезки пересвета после баланса белого и источник белого для основы тона.
/// Выводится из основы при разрешении параметров, отдельной ручки нет (решение 177).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhitePolicy {
    /// Обрезка по `SENSOR_CLIP_LEVEL` (1,0), белый основы — 1: прежнее поведение.
    SensorLevel,
    /// Обрезка по своему белому каждой точки, белый точки доходит до основы (AgX, ACES, filmic).
    PerPoint,
}

impl ResolvedToneBase {
    /// Какая это модель (для сообщений об отказе и выбора на GPU).
    pub fn kind(&self) -> ToneModelKind {
        match self {
            Self::Legacy => ToneModelKind::Legacy,
            Self::Agx(_) => ToneModelKind::Agx,
            Self::Aces(_) => ToneModelKind::Aces,
            Self::Filmic(_) => ToneModelKind::Filmic,
        }
    }

    /// Рабочее пространство `W` разрешённой модели (см. [`ToneBase::working_primaries`]).
    pub fn working_primaries(&self, output_gamut: DisplayGamut) -> Primaries {
        match self {
            Self::Legacy => output_gamut.primaries(),
            Self::Agx(_) | Self::Aces(_) | Self::Filmic(_) => WORKING_PRIMARIES,
        }
    }

    /// Политика белого для этой основы.
    pub fn white_policy(&self) -> WhitePolicy {
        match self {
            Self::Legacy => WhitePolicy::SensorLevel,
            Self::Agx(_) | Self::Aces(_) | Self::Filmic(_) => WhitePolicy::PerPoint,
        }
    }

    /// Формулы пикселя модели, работающей в `W`; `None` у прежней основы (её цепочка — в `run_reference`).
    /// Единственная точка, где эталонная цепочка узнаёт модель: стадии над изображениями дальше общие.
    pub fn working_stage(&self) -> Option<&dyn WorkingToneBase> {
        match self {
            Self::Legacy => None,
            Self::Agx(base) => Some(base.as_ref()),
            Self::Aces(base) => Some(base.as_ref()),
            Self::Filmic(base) => Some(base.as_ref()),
        }
    }
}

#[cfg(test)]
#[path = "base_tests.rs"]
mod tests;
