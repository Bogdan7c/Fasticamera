//! Исполнитель источников белого на потоке рендера (white-balance 01, решение WB8).
//!
//! Держит экземпляры источников из реестра (свои на каждую сессию показа), кормит их кадрами и сверяет их
//! заявку с действующей заявкой указания владельца. Что делать с ответом, решает владелец белого в движке:
//! здесь ответ только публикуется в [`WhiteAnswerChannel`] с камерой кадра.
//!
//! # Правила
//! * На новый поток всем источникам зовётся `begin_stream`; источник заявку забыл, поэтому действующая заявка
//!   подаётся ЗАНОВО (трейт говорит «владелец выдаст новую», но владелец номера не меняет, пока ждёт, — подаём
//!   тот же номер). Исключение — заявка, на которую ответ уже опубликован: второго ответа не будет.
//! * На одну заявку публикуется ровно один ответ. Ответ не на действующую заявку не публикуется.
//! * Отказы без обращения к источнику: не зарегистрирован — `NotRegistered`; нужен вход кроме подсказок цвета —
//!   `InputNotProvided` (плитки и уменьшенный кадр рендер не готовит). Отказ `request` — `Busy` / `Unavailable`.
//! * Показания (WB33): после `begin_stream` — сразу (`NotMeasuredYet`: прежнее число на доске не остаётся), затем раз
//!   в `reading_every_frames` годных кадров и только если показание изменилось. Показание — не ответ: заявки у него
//!   нет, владелец его не видит, заявку и ответы оно не трогает. Источнику, чей вход рендер не готовит, показание —
//!   `InputNotProvided`, как и ответ.
//! * Кадр годен, если у него есть собственный результат, он освещён не светодиодом и снят повторяющимся
//!   запросом (кадры серии и снимка со вспышкой сюда не доходят вовсе: их отпускают до `plan_frame`).

use std::num::NonZeroU32;

use camera_contract::auto_exposure::SignalDelivery;
use camera_contract::auto_exposure_carry::CameraTag;
use camera_contract::capture_result::SensorTimestamp;
use camera_contract::control::ExposureTime;
use camera_contract::frame_passport::RawFramePassport;
use camera_contract::white::{
    WhiteAnswer, WhiteAnswerChannel, WhiteRequestId, WhiteSourceOutcome, WhiteSourceRequest, WhiteUnavailableReason,
};
use camera_contract::white_reading::{WhiteReading, WhiteReadingBoard, WhiteSourceReading};
use isp_model::color::dng::DngColorProfile;
use scene_estimation::white_source::{WhiteFrame, WhiteInputKind, WhiteRequestRefusal, WhiteSource, WhiteStreamFacts};

use super::WhiteBalanceSetupError;
use crate::viewfinder::WhiteSourceRegistry;
use crate::vulkan_backend::camera::frame_role::{frame_light, viewfinder_frame_role};

/// Счётчики исполнителя (для диагностики).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SourceCounters {
    /// Опубликовано ответов.
    pub answers_published: u64,
    /// Из них толчок движку не дошёл (получатель ещё не привязан; ответ в ящике лежит).
    pub answers_signal_not_bound: u64,
    /// Ответов источника отброшено: не на действующую заявку или повторных.
    pub replies_dropped: u64,
    /// Кадров, отданных источникам.
    pub frames_observed: u64,
    /// Опубликовано показаний (только изменившиеся и первые после начала потока).
    pub readings_published: u64,
}

/// Откуда брать показание при публикации.
#[derive(Debug, Clone, Copy)]
enum ReadingBasis {
    /// Спросить источники.
    FromSources,
    /// Потока нет: что бы ни помнил источник, измерено ничего не было.
    NothingMeasured,
}

/// Исполнитель источников.
pub struct SourceExecutor {
    sources: Vec<Box<dyn WhiteSource>>,
    answers: WhiteAnswerChannel,
    /// Доска показаний: пишем, читает экран.
    readings: WhiteReadingBoard,
    /// Раз в сколько годных кадров обновлять показания.
    reading_every_frames: NonZeroU32,
    /// Годных кадров с прошлой проверки показаний.
    frames_since_reading: u32,
    /// Что последним лежит на доске у каждого источника (по индексу в `sources`); `None` — ещё ничего на этом потоке.
    published_readings: Vec<Option<WhiteReading>>,
    /// Камера текущего потока; `None` — поток ещё не начинался.
    camera: Option<CameraTag>,
    /// Заявка, которую источники держат прямо сейчас.
    submitted: Option<WhiteRequestId>,
    /// Последняя заявка, на которую ответ опубликован.
    answered: Option<WhiteRequestId>,
    last_answer: Option<WhiteAnswer>,
    counters: SourceCounters,
}

impl SourceExecutor {
    /// Собирает источники реестра (ошибка фабрики — ошибка сборки сессии).
    pub fn new(
        registry: &WhiteSourceRegistry,
        answers: WhiteAnswerChannel,
        readings: WhiteReadingBoard,
        reading_every_frames: NonZeroU32,
    ) -> Result<Self, WhiteBalanceSetupError> {
        let sources = registry
            .instantiate()
            .map_err(|error| WhiteBalanceSetupError(error.to_string()))?;
        let mut executor = Self {
            published_readings: vec![None; sources.len()],
            sources,
            answers,
            readings,
            reading_every_frames,
            frames_since_reading: 0,
            camera: None,
            submitted: None,
            answered: None,
            last_answer: None,
            counters: SourceCounters::default(),
        };
        // Доска могла помнить показание прошлого исполнителя (пересоздан показ): до первого кадра нового потока
        // экрану нельзя отдавать чужое число. «Опубликовано» у нового исполнителя пусто, значит публикация состоится.
        executor.publish_readings(ReadingBasis::NothingMeasured);
        Ok(executor)
    }

    /// Новый поток: источники забывают накопленное, действующая заявка подастся заново.
    pub fn begin_stream(&mut self, camera: CameraTag, profile: Option<&DngColorProfile>) {
        let facts = WhiteStreamFacts { camera, profile };
        for source in &mut self.sources {
            source.begin_stream(&facts);
        }
        self.camera = Some(camera);
        self.submitted = None;
        // Показания прежнего потока забыты вместе с источниками: на доске сразу честное «ещё не мерил».
        self.published_readings.fill(None);
        self.frames_since_reading = 0;
        self.publish_changed_readings();
    }

    /// Сверяет действующую заявку указания с тем, что держат источники.
    pub fn sync_request(&mut self, wanted: Option<WhiteSourceRequest>) {
        let wanted_id = wanted.map(|request| request.id);
        if self.submitted == wanted_id {
            return;
        }
        if let Some(previous) = self.submitted.take() {
            for source in &mut self.sources {
                source.cancel(previous);
            }
        }
        let Some(request) = wanted else {
            return;
        };
        if self.answered == Some(request.id) {
            // Ответ на эту заявку уже ушёл владельцу (он ещё не закрыл её указанием): второго не будет.
            return;
        }
        self.submit(request);
    }

    fn submit(&mut self, request: WhiteSourceRequest) {
        let Some(index) = self.sources.iter().position(|source| source.id() == request.source) else {
            self.refuse(
                request,
                WhiteSourceOutcome::Unavailable {
                    reason: WhiteUnavailableReason::NotRegistered,
                },
            );
            return;
        };
        if self.sources[index].required_input() != WhiteInputKind::ColorHints {
            self.refuse(
                request,
                WhiteSourceOutcome::Unavailable {
                    reason: WhiteUnavailableReason::InputNotProvided,
                },
            );
            return;
        }
        match self.sources[index].request(request.id) {
            Ok(()) => self.submitted = Some(request.id),
            Err(WhiteRequestRefusal::Busy) => self.refuse(request, WhiteSourceOutcome::Busy),
            Err(WhiteRequestRefusal::Unavailable(reason)) => {
                self.refuse(request, WhiteSourceOutcome::Unavailable { reason });
            }
        }
    }

    /// Отказ без ответа источника: публикуется сразу, заявка отвечена.
    fn refuse(&mut self, request: WhiteSourceRequest, outcome: WhiteSourceOutcome) {
        self.publish(request.id, None, outcome);
    }

    /// Кадр потока годный кадр (см. модуль).
    pub fn observe(&mut self, passport: &RawFramePassport) {
        // Правило годности — то же, что у видоискателя: серия и снимок со вспышкой не показываются, светодиод не свет сцены.
        if !viewfinder_frame_role(passport).is_shown() || !frame_light(passport).feeds_viewfinder_smoothing() {
            return;
        }
        let Some(details) = passport.capture.details() else {
            return;
        };
        let frame = WhiteFrame {
            sensor_timestamp: passport.sensor_timestamp,
            exposure_time: details
                .result
                .exposure_time
                .and_then(|time| ExposureTime::new(time).ok()),
            sensitivity_iso: details.result.sensitivity_iso,
            color_hints: &details.color,
            tiles: None,
            thumbnail: None,
        };
        for source in &mut self.sources {
            source.observe(&frame);
        }
        self.counters.frames_observed += 1;
        self.frames_since_reading += 1;
        if self.frames_since_reading >= self.reading_every_frames.get() {
            self.frames_since_reading = 0;
            self.publish_changed_readings();
        }
    }

    /// Поток кончился (линза меняется, показ остановлен), нового ещё нет: источники хранят показание кончившегося
    /// потока до `begin_stream`, но экрану его отдавать нельзя — на доске «ещё не мерил».
    pub fn forget_stream_readings(&mut self) {
        self.frames_since_reading = 0;
        self.publish_readings(ReadingBasis::NothingMeasured);
    }

    /// Публикует показания тех источников, чьё показание отличается от лежащего на доске.
    fn publish_changed_readings(&mut self) {
        self.publish_readings(ReadingBasis::FromSources);
    }

    fn publish_readings(&mut self, basis: ReadingBasis) {
        for (source, published) in self.sources.iter().zip(self.published_readings.iter_mut()) {
            let reading = if source.required_input() == WhiteInputKind::ColorHints {
                match basis {
                    ReadingBasis::FromSources => source.reading(),
                    ReadingBasis::NothingMeasured => WhiteReading::NotMeasuredYet,
                }
            } else {
                // Рендер не готовит такому источнику вход: показать нечего, и говорим об этом прямо.
                WhiteReading::Unavailable {
                    reason: WhiteUnavailableReason::InputNotProvided,
                }
            };
            if *published == Some(reading) {
                continue;
            }
            self.readings.publish(WhiteSourceReading {
                source: source.id(),
                reading,
            });
            *published = Some(reading);
            self.counters.readings_published += 1;
        }
    }

    /// Забирает готовые ответы и публикует те, что относятся к действующей заявке.
    pub fn collect_answers(&mut self) {
        for index in 0..self.sources.len() {
            while let Some(reply) = self.sources[index].poll() {
                let current = self.submitted == Some(reply.request) && self.answered != Some(reply.request);
                if current {
                    self.publish(reply.request, reply.measured_frame, reply.outcome);
                } else {
                    self.counters.replies_dropped += 1;
                }
            }
        }
    }

    fn publish(
        &mut self,
        request: WhiteRequestId,
        measured_frame: Option<SensorTimestamp>,
        outcome: WhiteSourceOutcome,
    ) {
        let Some(camera) = self.camera else {
            // Поток не начинался: отвечать от имени камеры нечем. Недостижимо (sync идёт после begin_stream).
            self.counters.replies_dropped += 1;
            return;
        };
        let answer = WhiteAnswer {
            request,
            camera,
            measured_frame,
            outcome,
        };
        let (_, delivery) = self.answers.publish(answer);
        if delivery == SignalDelivery::NotBoundYet {
            self.counters.answers_signal_not_bound += 1;
        }
        self.counters.answers_published += 1;
        self.answered = Some(request);
        self.last_answer = Some(answer);
    }

    /// Последний опубликованный ответ.
    pub fn last_answer(&self) -> Option<WhiteAnswer> {
        self.last_answer
    }

    /// Счётчики.
    pub fn counters(&self) -> SourceCounters {
        self.counters
    }
}
