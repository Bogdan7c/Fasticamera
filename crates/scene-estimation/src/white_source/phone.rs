//! Источник «телефон»: белая штатной автоматики по подсказкам цвета из результата кадра.
//!
//! Автоматика жива благодаря пустому вспомогательному потоку (решение 62 foundation). Её пикселей здесь нет: берётся
//! только ответ «какой здесь свет» — усиления каналов (`COLOR_CORRECTION_GAINS`), запасной путь — нейтральная точка.
//!
//! # Когда ответ годен (факты записей OnePlus 11, 92 открытия)
//! * `CONTROL_AWB_STATE` бесполезен: «сошёлся» стоит с нулевого кадра всегда, даже при мёртвой автоматике.
//! * Первые ≈ 5 кадров потока несут белую **прошлого сеанса** (возможно, другого света), побитово неподвижную.
//!   Значение, которое хоть раз сдвинулось, — уже измеренное на этом потоке (*свежее*): в нормальном свете сдвиг
//!   есть всегда (5–7-й кадр), даже если свет тот же, — оценка автоматики чуть дрожит. Нет сдвига — нет измерения
//!   (слишком темно), и отдать такое значение значило бы отдать вчерашний свет.
//! * Свежее значение ещё доползает, поэтому ждём [`PhoneWhiteConfig::stable_frames`] ровных кадров, но не дольше
//!   кадра [`PhoneWhiteConfig::settle_by_frame`]: дальше отдаём как есть с пониженной уверенностью.
//! * Ровно единичные усиления — мёртвая автоматика (сессия без вспомогательного потока), а не «зелёная белая».

use std::collections::VecDeque;

use camera_contract::capture_result::SensorTimestamp;
use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use camera_contract::frame_passport::CaptureColorHints;
use camera_contract::white::{
    SceneIlluminant, WhiteConfidence, WhiteRequestId, WhiteSourceFailure, WhiteSourceId, WhiteSourceOutcome,
    WhiteUnavailableReason, WhiteUnknownReason,
};
use camera_contract::white_reading::WhiteReading;
use isp_model::color::dng::{CameraNeutral, ColorProfileError, DngColorProfile};
use isp_model::color::white_point::{CctDuv, WhitePointError, cct_duv_from_chromaticity};
use serde::{Deserialize, Serialize};

use super::{WhiteFrame, WhiteInputKind, WhiteReply, WhiteRequestRefusal, WhiteSource, WhiteStreamFacts};
use crate::error::ConfigError;

/// Числа источника «телефон».
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PhoneWhiteConfig {
    /// Сколько свежих кадров подряд должны совпасть в пределах допуска.
    pub stable_frames: u32,
    /// Допуск совпадения: относительная разница отношений каналов R/G и B/G.
    pub stable_relative_tolerance: f64,
    /// Номер кадра потока, к которому свежее, но ещё не ровное значение отдаётся как есть.
    pub settle_by_frame: u32,
    /// Уверенность такого ответа.
    pub unsettled_confidence: f32,
    /// Через сколько кадров потока без единого сдвига ответ — «не знаю» (автоматика не меряет).
    pub give_up_frames: u32,
}

impl Default for PhoneWhiteConfig {
    /// `[estimate]` по записям зонда 18–19 и 24.11.2026 (79 открытий в нормальном свете): 6 кадров и 0,8 % —
    /// медиана ответа на 10-м кадре, ошибка к 90-му кадру 0,2 % (медиана) и 1,9 % (90 % открытий); 18 — внутри
    /// предела скрытия 20 кадров с запасом на доставку ответа движку и обратно; 150 кадров ≈ 2,5 с — внутри срока
    /// ожидания владельца 3 с. Записи — со штатной и постоянной экспозицией, у продукта экспозиция своя.
    fn default() -> Self {
        Self {
            stable_frames: 6,
            stable_relative_tolerance: 0.008,
            settle_by_frame: 18,
            unsettled_confidence: 0.5,
            give_up_frames: 150,
        }
    }
}

impl PhoneWhiteConfig {
    /// Проверяет числа.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let invalid = |field: &'static str, reason: &str| {
            Err(ConfigError::Invalid {
                field,
                reason: reason.to_owned(),
            })
        };
        if self.stable_frames == 0 {
            return invalid("phone.stable_frames", "нужно ≥ 1");
        }
        if !(self.stable_relative_tolerance.is_finite() && self.stable_relative_tolerance > 0.0) {
            return invalid("phone.stable_relative_tolerance", "нужно конечное > 0");
        }
        if self.settle_by_frame < self.stable_frames {
            return invalid("phone.settle_by_frame", "нужно не меньше stable_frames");
        }
        if !(self.unsettled_confidence.is_finite() && (0.0..=1.0).contains(&self.unsettled_confidence)) {
            return invalid("phone.unsettled_confidence", "нужно [0, 1]");
        }
        if self.give_up_frames < self.settle_by_frame {
            return invalid("phone.give_up_frames", "нужно не меньше settle_by_frame");
        }
        Ok(())
    }
}

/// Почему подсказка кадра не стала белой.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum HintWhiteError {
    /// Подсказок нет, они непригодны или автоматика мертва (единичные усиления).
    #[error("автоматика телефона белую не называет")]
    AutomationSilent,
    /// Профиль цвета не дал белую точку для нейтрали.
    #[error("нейтраль телефона: {0}")]
    Profile(#[from] ColorProfileError),
    /// Белая точка не переводится в «температура + Duv».
    #[error("белая точка телефона: {0}")]
    WhitePoint(#[from] WhitePointError),
}

/// Белая телефона по подсказкам одного кадра, без суждения о свежести (его выносит [`PhoneWhiteSource`]).
pub fn white_of_hints(hints: &CaptureColorHints, profile: &DngColorProfile) -> Result<CctDuv, HintWhiteError> {
    let neutral = phone_neutral(hints).ok_or(HintWhiteError::AutomationSilent)?;
    Ok(cct_duv_from_chromaticity(profile.white_for_neutral(neutral)?.white)?)
}

/// Нейтраль телефона: сначала усиления (`N_c = 1/g_c`, зелёный — среднее двух), затем нейтральная точка.
/// `None` — подсказок нет или автоматика мертва.
fn phone_neutral(hints: &CaptureColorHints) -> Option<CameraNeutral> {
    let from_gains = hints.color_gains.and_then(|gains| {
        let [red, green_even, green_odd, blue] = gains.0.map(f64::from);
        CameraNeutral::new([1.0 / red, 2.0 / (green_even + green_odd), 1.0 / blue]).ok()
    });
    let neutral = match from_gains {
        Some(neutral) => neutral,
        None => {
            let point = hints.neutral_color_point?;
            CameraNeutral::new([point[0].to_f64()?, point[1].to_f64()?, point[2].to_f64()?]).ok()?
        }
    };
    let [red, green, blue] = neutral.values();
    // Свет, при котором сенсор без баланса даёт серое, у камер с зелёным перевесом не бывает: это «баланса не считали».
    let automation_is_dead = red == green && blue == green;
    (!automation_is_dead).then_some(neutral)
}

/// Отношения каналов R/G и B/G нейтрали: то, что сравнивается между кадрами.
type ChannelRatios = [f64; 2];

fn ratios_of(neutral: CameraNeutral) -> ChannelRatios {
    let [red, green, blue] = neutral.values();
    [red / green, blue / green]
}

fn close_enough(left: ChannelRatios, right: ChannelRatios, tolerance: f64) -> bool {
    left.iter()
        .zip(right)
        .all(|(left, right)| (left / right - 1.0).abs() <= tolerance)
}

/// Свежесть значения автоматики на текущем потоке.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Freshness {
    /// Подсказки ещё не приходили.
    NoHintYet,
    /// Значение не сдвинулось от первого кадра потока: белая прошлого сеанса.
    LeftOver { first: ChannelRatios },
    /// Значение сдвигалось: измерено на этом потоке.
    Measured,
}

/// Что накоплено по текущему потоку.
#[derive(Debug)]
struct StreamState {
    profile: Option<DngColorProfile>,
    freshness: Freshness,
    /// Годных кадров потока.
    frames: u32,
    /// Последние свежие значения (не больше `stable_frames`).
    recent: VecDeque<ChannelRatios>,
    /// Последний кадр с подсказкой.
    last: Option<(SensorTimestamp, CaptureColorHints)>,
}

impl StreamState {
    fn new(profile: Option<DngColorProfile>) -> Self {
        Self {
            profile,
            freshness: Freshness::NoHintYet,
            frames: 0,
            recent: VecDeque::new(),
            last: None,
        }
    }
}

/// Источник «телефон» (см. модуль).
pub struct PhoneWhiteSource {
    config: PhoneWhiteConfig,
    stream: Option<StreamState>,
    pending: Option<WhiteRequestId>,
    ready: Option<WhiteReply>,
}

impl PhoneWhiteSource {
    /// Источник с проверенными числами.
    pub fn new(config: PhoneWhiteConfig) -> Result<Self, ConfigError> {
        config.validate()?;
        Ok(Self {
            config,
            stream: None,
            pending: None,
            ready: None,
        })
    }

    /// Учитывает подсказку кадра: свежесть и окно последних значений.
    fn note_hint(stream: &mut StreamState, config: &PhoneWhiteConfig, frame: &WhiteFrame<'_>, neutral: CameraNeutral) {
        let ratios = ratios_of(neutral);
        stream.freshness = match stream.freshness {
            Freshness::NoHintYet => Freshness::LeftOver { first: ratios },
            // Сравнение точное намеренно: значение прошлого сеанса автоматика повторяет побитово.
            Freshness::LeftOver { first } if first == ratios => Freshness::LeftOver { first },
            Freshness::LeftOver { .. } | Freshness::Measured => Freshness::Measured,
        };
        if stream.freshness == Freshness::Measured {
            stream.recent.push_back(ratios);
            while stream.recent.len() > config.stable_frames as usize {
                stream.recent.pop_front();
            }
        }
        stream.last = Some((frame.sensor_timestamp, *frame.color_hints));
    }

    /// Есть ли что ответить на заявку по накопленному; `None` — ещё ждать.
    fn verdict(&self, stream: &StreamState) -> Option<(Option<SensorTimestamp>, WhiteSourceOutcome)> {
        let Some((timestamp, hints)) = stream.last else {
            // Подсказок нет ни у одного кадра: к сроку устоявшегося ответа это уже не «ещё не пришли».
            return (stream.frames >= self.config.settle_by_frame).then_some((
                None,
                WhiteSourceOutcome::Unavailable {
                    reason: WhiteUnavailableReason::AutomationSilent,
                },
            ));
        };
        if stream.freshness != Freshness::Measured {
            return (stream.frames >= self.config.give_up_frames).then_some((
                Some(timestamp),
                WhiteSourceOutcome::Unknown {
                    reason: WhiteUnknownReason::NoFreshMeasurement,
                },
            ));
        }
        let confidence = self.confidence(stream)?;
        let outcome = match &stream.profile {
            None => WhiteSourceOutcome::Unavailable {
                reason: WhiteUnavailableReason::NoColorProfile,
            },
            Some(profile) => ready_outcome(&hints, profile, confidence),
        };
        Some((Some(timestamp), outcome))
    }

    /// Насколько устоялось свежее значение; `None` — ещё не устоялось и срок не вышел.
    fn confidence(&self, stream: &StreamState) -> Option<WhiteConfidence> {
        let window_full = stream.recent.len() >= self.config.stable_frames as usize;
        let steady = stream.recent.back().is_some_and(|last| {
            stream
                .recent
                .iter()
                .all(|value| close_enough(*value, *last, self.config.stable_relative_tolerance))
        });
        if window_full && steady {
            return Some(WhiteConfidence::FULL);
        }
        (stream.frames >= self.config.settle_by_frame)
            .then(|| WhiteConfidence::from_fraction(self.config.unsettled_confidence))
    }

    /// Отвечает на действующую заявку, если уже есть что.
    fn answer_pending(&mut self) {
        let (Some(request), Some(stream)) = (self.pending, self.stream.as_ref()) else {
            return;
        };
        if let Some((measured_frame, outcome)) = self.verdict(stream) {
            self.pending = None;
            self.ready = Some(WhiteReply {
                request,
                measured_frame,
                outcome,
            });
        }
    }
}

/// Белая подсказки кадра в словаре настроек; `None` — подсказка не переводится (одна математика у ответа и показания).
fn setting_of_hints(hints: &CaptureColorHints, profile: &DngColorProfile) -> Option<WhiteBalanceSetting> {
    let white = white_of_hints(hints, profile).ok()?;
    let temperature = ColorTemperature::from_kelvin(white.temperature.0).ok()?;
    let tint = Tint::from_duv(white.duv.0).ok()?;
    Some(WhiteBalanceSetting { temperature, tint })
}

/// Готовый ответ по подсказке кадра или честная поломка.
fn ready_outcome(
    hints: &CaptureColorHints,
    profile: &DngColorProfile,
    confidence: WhiteConfidence,
) -> WhiteSourceOutcome {
    match setting_of_hints(hints, profile) {
        Some(setting) => WhiteSourceOutcome::Ready {
            illuminant: SceneIlluminant::Uniform(setting),
            confidence,
        },
        None => WhiteSourceOutcome::Failed {
            failure: WhiteSourceFailure::HintNotConvertible,
        },
    }
}

impl PhoneWhiteSource {
    /// Показание по накопленному на потоке (см. [`WhiteSource::reading`]).
    fn reading_of(&self, stream: &StreamState) -> WhiteReading {
        let Some((_, hints)) = stream.last else {
            // Подсказок нет вовсе: пока срок «устоявшегося» не вышел — «ещё не мерил», после — автоматика молчит.
            return if stream.frames >= self.config.settle_by_frame {
                WhiteReading::Unavailable {
                    reason: WhiteUnavailableReason::AutomationSilent,
                }
            } else {
                WhiteReading::NotMeasuredYet
            };
        };
        if stream.freshness != Freshness::Measured {
            // Значение не сдвигалось с первого кадра: белая прошлого сеанса, а не свет сцены.
            return WhiteReading::NotMeasuredYet;
        }
        let Some(profile) = &stream.profile else {
            return WhiteReading::Unavailable {
                reason: WhiteUnavailableReason::NoColorProfile,
            };
        };
        match setting_of_hints(&hints, profile) {
            Some(white) => WhiteReading::Measured { white },
            None => WhiteReading::Failed {
                failure: WhiteSourceFailure::HintNotConvertible,
            },
        }
    }
}

impl WhiteSource for PhoneWhiteSource {
    fn id(&self) -> WhiteSourceId {
        WhiteSourceId::PHONE
    }

    fn required_input(&self) -> WhiteInputKind {
        WhiteInputKind::ColorHints
    }

    fn begin_stream(&mut self, facts: &WhiteStreamFacts<'_>) {
        self.stream = Some(StreamState::new(facts.profile.cloned()));
        self.pending = None;
        self.ready = None;
    }

    fn observe(&mut self, frame: &WhiteFrame<'_>) {
        let Some(stream) = self.stream.as_mut() else {
            return;
        };
        stream.frames = stream.frames.saturating_add(1);
        if let Some(neutral) = phone_neutral(frame.color_hints) {
            Self::note_hint(stream, &self.config, frame, neutral);
        }
        self.answer_pending();
    }

    fn request(&mut self, request: WhiteRequestId) -> Result<(), WhiteRequestRefusal> {
        // Прежняя заявка у этого источника ничего не стоит: новая её заменяет, «занят» не бывает.
        self.pending = Some(request);
        self.ready = None;
        self.answer_pending();
        Ok(())
    }

    fn cancel(&mut self, request: WhiteRequestId) {
        if self.pending == Some(request) {
            self.pending = None;
        }
        if self.ready.is_some_and(|reply| reply.request == request) {
            self.ready = None;
        }
    }

    fn poll(&mut self) -> Option<WhiteReply> {
        self.ready.take()
    }

    fn reading(&self) -> WhiteReading {
        // Потока нет — измерять нечем (и прежней белой тут быть не может: поток забывает всё).
        self.stream
            .as_ref()
            .map_or(WhiteReading::NotMeasuredYet, |stream| self.reading_of(stream))
    }
}

#[cfg(test)]
#[path = "phone_tests.rs"]
mod tests;
