//! Белая на потоке рендера (white-balance 01; прежний AWB сессии 08 в продукт больше не идёт).
//!
//! # Кто чем владеет
//! * [`WhiteBalanceRunner`] — единственный, кто решает, какая белая идёт в ISP, показывать ли кадр
//!   (холодный старт); расписание статистики кадра — не здесь (`statistics_schedule`);
//! * `applied::AppliedWhite` — применяемая белая: цель владельца, переход по меткам кадров, побитовая цель после него;
//! * `sources::SourceExecutor` — источники белого из реестра: заявка владельца → ответ в ящик движку;
//! * `cold_start::ColdStart` — скрытие кадров, пока действует заявка `HideFrames`;
//! * `DirectiveSlot<WhiteDirective>` — указания владельца белого (`camera-core`).
//!
//! # Поток данных
//! `plan_frame` на каждый НОВЫЙ показываемый кадр камеры (до параметров ISP): свежее указание → привязка к
//! потоку (источникам `begin_stream`) → сверка заявки и кадр источникам → решение холодного старта → шаг
//! применяемой белой. Статистика кадров белой не питает: её считает расписание ради AE, белую видоискателя
//! ведёт владелец белого, а белая кадра со вспышкой — постоянный цвет светодиода из плана движка (WB11).
//!
//! # Инварианты
//! 1. В ISP идёт только белая из указания владельца (или белая стенда без движка, пока указаний не было вовсе). Подсказки
//!    HAL — лишь диагностика. Без нового указания белая не меняется ни на бит.
//! 2. Скрытие кадров ограничено (`max_hidden_frames`) — картинка не прячется вечно.

pub(in crate::vulkan_backend::camera) mod applied;
mod cold_start;
mod diagnostics;
mod hal_white;
mod sources;

use std::num::NonZeroU32;
use std::sync::Arc;

use camera_contract::auto_exposure_carry::CameraTag;
use camera_contract::frame_passport::{RawFramePassport, StreamPassport};
use camera_contract::three_a::DirectiveGeneration;
use camera_contract::white::{WhileAwaitingWhite, WhiteDirective, WhiteSourceRequest, WhiteTransition};
use isp_model::color::chromaticity::Chromaticity;
use isp_model::color::dng::DngColorProfile;
use isp_model::color::white_point::CctDuv;

use self::applied::AppliedWhite;
use self::cold_start::{ColdStart, ColdStartVerdict};
use self::hal_white::HalWhiteTracker;
use self::sources::SourceExecutor;
use super::pipelines::IspPresentation;
use crate::config::WhiteBalanceConfig;
use crate::viewfinder::{ColdStartState, WhiteLinks};

const LOG_TARGET: &str = "fasticamera::render::white_balance";

/// Поколение потока у AWB: растёт с каждым новым потоком камеры.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AwbStream(pub u64);

/// Что белая даёт ISP и показу для одного нового кадра камеры.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FramePlan {
    /// Поток, к которому относится кадр (метка для статистики слота).
    pub stream: AwbStream,
    /// Белая для параметров ISP этого кадра.
    pub white: Chromaticity,
    /// Показывать ли обработанный кадр.
    pub presentation: IspPresentation,
}

/// Из чего собирается белая сессии показа.
#[derive(Clone)]
pub struct WhiteBalanceParts {
    /// Настройки.
    pub config: WhiteBalanceConfig,
    /// Связка с владельцем белого: указания, ящик ответов, источники.
    pub links: WhiteLinks,
}

/// Белую не удалось собрать (конфиг отвергнут или фабрика источника отказала).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("настройки AWB: {0}")]
pub struct WhiteBalanceSetupError(pub String);

/// Привязка к потоку: профиль цвета считается один раз на поток.
struct BoundStream {
    passport: Arc<StreamPassport>,
    id: AwbStream,
    profile: Option<DngColorProfile>,
}

/// Счётчики runner.
#[derive(Default)]
struct RunnerCounters {
    hidden_frames: u64,
    forced: u64,
    frames_without_directive: u64,
}

/// Белая одной сессии показа (см. модуль).
pub struct WhiteBalanceRunner {
    directives: camera_contract::three_a::DirectiveSlot<WhiteDirective>,
    applied_generation: Option<DirectiveGeneration>,
    /// Действующее указание: из него берётся заявка источнику.
    last_directive: Option<WhiteDirective>,
    applied: AppliedWhite,
    sources: SourceExecutor,
    cold_start: ColdStart,
    bound: Option<BoundStream>,
    next_stream_id: u64,
    hal: HalWhiteTracker,
    hal_white: Option<CctDuv>,
    counters: RunnerCounters,
}

impl WhiteBalanceRunner {
    /// Собирает белую сессии показа: свежие источники из реестра.
    pub fn new(parts: WhiteBalanceParts) -> Result<Self, WhiteBalanceSetupError> {
        let WhiteBalanceParts { config, links } = parts;
        let setup = |error: &dyn std::fmt::Display| WhiteBalanceSetupError(error.to_string());
        let reading_every_frames = NonZeroU32::new(config.reading_every_frames)
            .ok_or_else(|| setup(&"reading_every_frames должен быть не меньше 1"))?;
        let sources = SourceExecutor::new(
            &links.sources,
            links.answers.clone(),
            links.readings.clone(),
            reading_every_frames,
        )?;
        Ok(Self {
            directives: links.directives,
            applied_generation: None,
            last_directive: None,
            applied: AppliedWhite::default(),
            sources,
            cold_start: ColdStart::new(config.max_hidden_frames),
            bound: None,
            next_stream_id: 0,
            hal: HalWhiteTracker::default(),
            hal_white: None,
            counters: RunnerCounters::default(),
        })
    }

    /// Поток камеры неактивен: следующий кадр — уже другой поток. Применённая белая остаётся.
    pub fn forget_stream(&mut self) {
        self.bound = None;
        self.sources.forget_stream_readings();
    }

    /// План для нового кадра камеры (см. модуль). Вызывается до подготовки параметров ISP.
    pub fn plan_frame(&mut self, passport: &RawFramePassport) -> FramePlan {
        self.refresh_directive();
        let request = self.request();
        let stream = self.bind_stream(passport);
        self.sources.sync_request(request);
        self.sources.observe(passport);
        self.sources.collect_answers();
        let cold_start_before = self.cold_start.state();
        let verdict = self.cold_start.decide(request.map(|request| request.while_awaiting));
        let shown = verdict != ColdStartVerdict::Hide;
        let applied = self.applied.advance(passport.sensor_timestamp.as_nanos(), shown);
        self.note_cold_start(cold_start_before, verdict);
        self.note_hal(passport);
        if applied.without_directive {
            self.counters.frames_without_directive += 1;
        }
        FramePlan {
            stream,
            white: applied.white,
            presentation: if shown {
                IspPresentation::Show
            } else {
                IspPresentation::HideUntilColorKnown
            },
        }
    }

    // ---- внутреннее ----

    /// Заявка источнику действующего указания.
    fn request(&self) -> Option<WhiteSourceRequest> {
        self.last_directive.and_then(|directive| directive.request)
    }

    /// Свежее указание владельца (если появилось): применяется один раз на поколение. Первое указание сессии
    /// показа — «последнее из ящика», то есть и указание, опубликованное до создания сессии.
    fn refresh_directive(&mut self) {
        let Some((generation, directive)) = self.directives.newer_than(self.applied_generation) else {
            return;
        };
        let notable = directive_is_notable(self.last_directive.as_ref(), &directive);
        self.applied.accept(&directive);
        self.last_directive = Some(directive);
        self.applied_generation = Some(generation);
        if notable {
            tracing::info!(target: LOG_TARGET, generation = generation.0, ?directive, "указание белой применено");
        } else {
            // Рука на шкале или вопрос источнику при видимой картинке (слежение спрашивает десять раз в секунду).
            tracing::debug!(target: LOG_TARGET, generation = generation.0, ?directive, "указание белой применено");
        }
    }

    /// Привязывает к потоку кадра; новый поток строит профиль цвета и сообщает источникам.
    fn bind_stream(&mut self, passport: &RawFramePassport) -> AwbStream {
        if let Some(bound) = &self.bound
            && Arc::ptr_eq(&bound.passport, &passport.stream)
        {
            return bound.id;
        }
        self.next_stream_id += 1;
        let id = AwbStream(self.next_stream_id);
        let stream = &passport.stream;
        let profile = match DngColorProfile::from_calibration_sets(&stream.color_calibration) {
            Ok(profile) => Some(profile),
            Err(error) => {
                // Без профиля источники работают без перевода в камерные значения, а HAL-сравнение не строится.
                tracing::warn!(target: LOG_TARGET, %error, "профиль цвета потока не построен");
                None
            }
        };
        self.sources
            .begin_stream(CameraTag::of(&stream.camera), profile.as_ref());
        self.hal.reset();
        self.hal_white = None;
        tracing::info!(target: LOG_TARGET, stream = id.0, "новый поток белой");
        self.bound = Some(BoundStream {
            passport: Arc::clone(stream),
            id,
            profile,
        });
        id
    }

    /// Счётчики и журнал холодного старта (журнал — только на границах состояния, не на кадр).
    fn note_cold_start(&mut self, before: ColdStartState, verdict: ColdStartVerdict) {
        match verdict {
            ColdStartVerdict::Hide => self.counters.hidden_frames += 1,
            ColdStartVerdict::ShowForced { hidden } => {
                self.counters.forced += 1;
                tracing::warn!(
                    target: LOG_TARGET,
                    hidden,
                    "холодный старт: ответа источника нет, кадры показаны с текущей белой"
                );
            }
            ColdStartVerdict::Show => {
                if let ColdStartState::Pending { hidden } = before {
                    tracing::info!(target: LOG_TARGET, hidden, "холодный старт закончен закрытием заявки");
                }
            }
        }
    }

    /// Белая HAL для сравнения (только диагностика).
    fn note_hal(&mut self, passport: &RawFramePassport) {
        let Some(details) = passport.capture.details() else {
            return;
        };
        let Some(profile) = self.bound.as_ref().and_then(|bound| bound.profile.as_ref()) else {
            return;
        };
        self.hal_white = self.hal.observe(&details.color, profile);
    }
}

/// Стоит ли указание строки `info`: первое указание, начало или конец скрытия кадров и плавный переезд белой. Шаги
/// ползунка (до 120 в секунду) и вопросы источнику при видимой картинке в `info` не идут: итог заявки человека
/// называет журнал движка.
fn directive_is_notable(previous: Option<&WhiteDirective>, next: &WhiteDirective) -> bool {
    let Some(previous) = previous else {
        return true;
    };
    let hides_frames = |directive: &WhiteDirective| {
        directive
            .request
            .is_some_and(|request| request.while_awaiting == WhileAwaitingWhite::HideFrames)
    };
    let moves_smoothly = previous.white != next.white && next.transition != WhiteTransition::Immediate;
    moves_smoothly || hides_frames(previous) != hides_frames(next)
}

#[cfg(test)]
#[path = "applied_tests.rs"]
mod applied_tests;
#[cfg(test)]
#[path = "runner_tests.rs"]
mod runner_tests;
#[cfg(test)]
#[path = "sources_tests.rs"]
mod sources_tests;
#[cfg(test)]
pub(in crate::vulkan_backend::camera) mod test_support;
