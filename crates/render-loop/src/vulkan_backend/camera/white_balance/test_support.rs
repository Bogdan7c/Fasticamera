//! Общие помощники юнит-тестов белой (без GPU): паспорта потоков и кадров, раннер, фальшивый источник белого с
//! журналом вызовов.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение — сигнал провала"
)]

use camera_contract::capabilities::{
    ColorCalibrationSet, LensShading, RawSupport, SensorLevels, VendorProcessingFlags,
};
use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use camera_contract::fixtures::{camera_id, main_wide_camera};
use camera_contract::frame_passport::{
    CaptureColorHints, ColorCorrectionGains, FrameCaptureDetails, FrameCaptureMeta, FrameSequence,
};
use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::raw_format::{CfaLayout, MosaicKind, MosaicPattern, RawFormat};
use camera_contract::stream::{FrameRateMechanism, ReadoutAspect, TargetFrameRate};
use camera_contract::three_a::{RequestOrigin, ThreeAResultState};
use camera_contract::white::{
    WhileAwaitingWhite, WhiteDirective, WhiteRequestId, WhiteSourceId, WhiteSourceOutcome, WhiteSourceRequest,
    WhiteTransition,
};
use camera_contract::white_reading::WhiteReading;
use isp_model::color::temperature::Kelvin;
use isp_model::color::white_point::{CctDuv, Duv, cct_duv_from_chromaticity, chromaticity_from_cct_duv};
use scene_estimation::white_source::{
    WhiteFrame, WhiteInputKind, WhiteReply, WhiteRequestRefusal, WhiteSource, WhiteStreamFacts,
};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use super::applied::WHITE_WITHOUT_DIRECTIVE;
use super::*;
use crate::viewfinder::{WhiteLinks, WhiteSourceRegistry};

pub(super) const FRAME_NANOS: i64 = 16_666_667;

fn calibration() -> Vec<ColorCalibrationSet> {
    match main_wide_camera().capabilities.raw {
        RawSupport::Supported(info) => info.color_calibration,
        RawSupport::NotSupported => panic!("в образце основной камеры есть RAW"),
    }
}

/// Новый поток (каждый вызов — новый `Arc`, то есть новая сессия камеры).
pub(in crate::vulkan_backend::camera) fn new_stream() -> Arc<StreamPassport> {
    Arc::new(StreamPassport {
        camera: camera_id("2"),
        raw_format: RawFormat::Raw16,
        buffer_size: PixelSize::new(64, 48),
        readout: ReadoutAspect::FourByThree,
        valid_area: PixelRect {
            left: 0,
            top: 0,
            size: PixelSize::new(64, 48),
        },
        sensor_area: camera_contract::lens_shading::ValidAreaOnSensor::whole_active_array(PixelSize::new(64, 48)),
        lens_shading: camera_contract::lens_shading::LensShadingCalibrationState::NotCalibrated(
            camera_contract::lens_shading::NotCalibratedReason::NoCalibrationForCamera,
        ),
        mosaic: MosaicPattern {
            kind: MosaicKind::Bayer,
            layout: CfaLayout::Bggr,
        },
        static_levels: SensorLevels {
            black_level_pattern: [64; 4],
            white_level: 1023,
        },
        vendor_processing: VendorProcessingFlags {
            lens_shading: LensShading::AppliedByVendor,
        },
        sensor_orientation: SensorOrientation::Degrees90,
        frame_rate: TargetFrameRate::new(60).expect("60 к/с"),
        frame_rate_mechanism: FrameRateMechanism::HalDefault,
        color_calibration: calibration(),
    })
}

pub(super) fn frame_at(stream: &Arc<StreamPassport>, index: i64, color: CaptureColorHints) -> RawFramePassport {
    RawFramePassport {
        stream: Arc::clone(stream),
        sequence: FrameSequence(u64::try_from(index).expect("номер кадра")),
        sensor_timestamp: SensorTimestamp::from_nanos(1_000_000_000 + index * FRAME_NANOS),
        capture: FrameCaptureMeta::Paired(FrameCaptureDetails {
            result: CaptureResultMeta {
                frame_number: FrameNumber(index),
                sensor_timestamp: Some(SensorTimestamp::from_nanos(1_000_000_000 + index * FRAME_NANOS)),
                exposure_time: None,
                sensitivity_iso: None,
                reported_frame_duration: None,
                rolling_shutter_skew: None,
                three_a: ThreeAResultState::unreported(RequestOrigin::Repeating),
            },
            color,
        }),
    }
}

pub(in crate::vulkan_backend::camera) fn frame(stream: &Arc<StreamPassport>, index: i64) -> RawFramePassport {
    frame_at(stream, index, CaptureColorHints::default())
}

// ---- стенд ----

/// Раннер и связка с «владельцем» (его роль в тестах играет тест: публикует указания и читает ответы).
pub(super) fn runner_with(
    config: WhiteBalanceConfig,
    sources: WhiteSourceRegistry,
) -> (WhiteBalanceRunner, WhiteLinks) {
    let links = WhiteLinks {
        sources,
        ..WhiteLinks::default()
    };
    let runner = WhiteBalanceRunner::new(WhiteBalanceParts {
        config,
        links: links.clone(),
    })
    .expect("белая собирается");
    (runner, links)
}

/// Раннер с настоящим источником «телефон».
pub(super) fn runner() -> (WhiteBalanceRunner, WhiteLinks) {
    let registry = WhiteSourceRegistry::with_phone(Default::default()).expect("числа источника верны");
    runner_with(WhiteBalanceConfig::default(), registry)
}

/// Раннер без источников.
pub(super) fn runner_without_sources() -> (WhiteBalanceRunner, WhiteLinks) {
    runner_with(WhiteBalanceConfig::default(), WhiteSourceRegistry::new())
}

pub(super) fn kelvin_of(white: Chromaticity) -> f64 {
    cct_duv_from_chromaticity(white)
        .expect("в таблице Робертсона")
        .temperature
        .0
}

/// Белая цель «температура + нулевой Duv».
pub(super) fn setting_of(kelvin: f64) -> WhiteBalanceSetting {
    WhiteBalanceSetting {
        temperature: ColorTemperature::from_kelvin(kelvin).expect("температура"),
        tint: Tint::NEUTRAL,
    }
}

/// Цветность, которую обязан отдать ISP для цели `kelvin` (побитово то, что даёт перевод цели).
pub(super) fn chromaticity_of(kelvin: f64) -> Chromaticity {
    chromaticity_from_cct_duv(CctDuv {
        temperature: Kelvin(kelvin),
        duv: Duv(0.0),
    })
    .expect("белая в таблице")
}

/// Заявка источнику.
pub(super) fn request_of(id: u64, source: WhiteSourceId, while_awaiting: WhileAwaitingWhite) -> WhiteSourceRequest {
    WhiteSourceRequest {
        id: WhiteRequestId(id),
        source,
        while_awaiting,
    }
}

/// Указание «держать эту белую», с переходом и заявкой.
pub(super) fn directive_of(
    kelvin: f64,
    transition: WhiteTransition,
    request: Option<WhiteSourceRequest>,
) -> WhiteDirective {
    WhiteDirective {
        white: setting_of(kelvin),
        transition,
        request,
    }
}

pub(super) fn over(millis: u64) -> WhiteTransition {
    WhiteTransition::Over(Duration::from_millis(millis))
}

/// Подсказки автоматики телефона для света `kelvin`: усиления, обратные нейтрали камеры.
pub(super) fn phone_hints(kelvin: f64) -> CaptureColorHints {
    let profile = DngColorProfile::from_calibration_sets(&calibration()).expect("профиль");
    let neutral = profile
        .neutral_for_white(chromaticity_of(kelvin))
        .expect("нейтраль")
        .values();
    let gain = |channel: f64| (1.0 / channel) as f32;
    CaptureColorHints {
        color_gains: Some(ColorCorrectionGains([gain(neutral[0]), 1.0, 1.0, gain(neutral[2])])),
        ..CaptureColorHints::default()
    }
}

/// Что делали с фальшивым источником.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Call {
    BeginStream,
    Observe(i64),
    Request(WhiteRequestId),
    Cancel(WhiteRequestId),
}

/// Общая «пульт» фальшивого источника: журнал вызовов, очередь готовых ответов и отказ на заявку.
#[derive(Default)]
pub(super) struct FakeSourceBoard {
    pub(super) calls: Mutex<Vec<Call>>,
    pub(super) replies: Mutex<VecDeque<WhiteReply>>,
    pub(super) refusal: Mutex<Option<WhiteRequestRefusal>>,
    /// Показание, которое отдаёт источник; `None` — «ещё не мерил». Начало потока его забывает (как у настоящего).
    pub(super) shown_reading: Mutex<Option<WhiteReading>>,
}

impl FakeSourceBoard {
    pub(super) fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    pub(super) fn count(&self, wanted: Call) -> usize {
        self.calls().iter().filter(|call| **call == wanted).count()
    }

    pub(super) fn requests(&self) -> Vec<WhiteRequestId> {
        self.calls()
            .into_iter()
            .filter_map(|call| if let Call::Request(id) = call { Some(id) } else { None })
            .collect()
    }

    pub(super) fn observed(&self) -> usize {
        self.calls()
            .iter()
            .filter(|call| matches!(call, Call::Observe(_)))
            .count()
    }

    pub(super) fn reply(&self, request: u64, outcome: WhiteSourceOutcome) {
        self.replies.lock().unwrap().push_back(WhiteReply {
            request: WhiteRequestId(request),
            measured_frame: None,
            outcome,
        });
    }
}

/// Фальшивый источник: ничего не считает, отвечает тем, что положил тест.
pub(super) struct FakeSource {
    pub(super) id: WhiteSourceId,
    pub(super) input: WhiteInputKind,
    pub(super) board: Arc<FakeSourceBoard>,
}

impl WhiteSource for FakeSource {
    fn id(&self) -> WhiteSourceId {
        self.id
    }

    fn required_input(&self) -> WhiteInputKind {
        self.input
    }

    fn begin_stream(&mut self, _stream: &WhiteStreamFacts<'_>) {
        self.board.calls.lock().unwrap().push(Call::BeginStream);
        *self.board.shown_reading.lock().unwrap() = None;
    }

    fn observe(&mut self, frame: &WhiteFrame<'_>) {
        self.board
            .calls
            .lock()
            .unwrap()
            .push(Call::Observe(frame.sensor_timestamp.as_nanos()));
    }

    fn request(&mut self, request: WhiteRequestId) -> Result<(), WhiteRequestRefusal> {
        self.board.calls.lock().unwrap().push(Call::Request(request));
        match *self.board.refusal.lock().unwrap() {
            Some(refusal) => Err(refusal),
            None => Ok(()),
        }
    }

    fn cancel(&mut self, request: WhiteRequestId) {
        self.board.calls.lock().unwrap().push(Call::Cancel(request));
    }

    fn poll(&mut self) -> Option<WhiteReply> {
        self.board.replies.lock().unwrap().pop_front()
    }

    fn reading(&self) -> WhiteReading {
        (*self.board.shown_reading.lock().unwrap()).unwrap_or(WhiteReading::NotMeasuredYet)
    }
}

/// Имя фальшивого источника.
pub(super) const FAKE_ID: WhiteSourceId = WhiteSourceId::new("fake");

/// Реестр с одним фальшивым источником, требующим вход `input`.
pub(super) fn fake_registry(input: WhiteInputKind) -> (WhiteSourceRegistry, Arc<FakeSourceBoard>) {
    let board = Arc::new(FakeSourceBoard::default());
    let mut registry = WhiteSourceRegistry::new();
    let shared = Arc::clone(&board);
    registry.register(move || {
        Ok(Box::new(FakeSource {
            id: FAKE_ID,
            input,
            board: Arc::clone(&shared),
        }) as Box<dyn WhiteSource>)
    });
    (registry, board)
}

/// Помощник: временная белая (для сравнения в тестах).
pub(super) const WITHOUT_DIRECTIVE: Chromaticity = WHITE_WITHOUT_DIRECTIVE;

/// Готовый ответ «свет такой-то».
pub(super) fn ready_outcome(kelvin: f64) -> WhiteSourceOutcome {
    WhiteSourceOutcome::Ready {
        illuminant: camera_contract::white::SceneIlluminant::Uniform(setting_of(kelvin)),
        confidence: camera_contract::white::WhiteConfidence::FULL,
    }
}
