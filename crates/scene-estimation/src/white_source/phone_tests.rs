//! Тесты источника «телефон»: ответ, его свежесть и отказы. Траектории усилений — с телефона (зонд, OnePlus 11).

use camera_contract::auto_exposure_carry::CameraTag;
use camera_contract::capabilities::RawSupport;
use camera_contract::fixtures::main_wide_camera;
use camera_contract::frame_passport::ColorCorrectionGains;
use camera_contract::ids::CameraId;
use isp_model::color::temperature::Kelvin;
use isp_model::color::white_point::{Duv, chromaticity_from_cct_duv};

use super::*;

const FRAME_NANOS: i64 = 16_666_667;

fn profile() -> DngColorProfile {
    let RawSupport::Supported(info) = main_wide_camera().capabilities.raw else {
        panic!("в образце основной камеры есть RAW");
    };
    DngColorProfile::from_calibration_sets(&info.color_calibration).expect("профиль цвета")
}

fn hints(red: f32, blue: f32) -> CaptureColorHints {
    CaptureColorHints {
        color_gains: Some(ColorCorrectionGains([red, 1.0, 1.0, blue])),
        ..CaptureColorHints::default()
    }
}

/// Усиления, которыми телефон назвал бы свет `kelvin` на оси Планка.
fn hints_for(kelvin: f64) -> CaptureColorHints {
    let white = chromaticity_from_cct_duv(CctDuv {
        temperature: Kelvin(kelvin),
        duv: Duv(0.0),
    })
    .expect("белая");
    let neutral = profile().neutral_for_white(white).expect("нейтраль").values();
    hints((1.0 / neutral[0]) as f32, (1.0 / neutral[2]) as f32)
}

/// Источник с потоком и счётчиком кадров.
struct Bench {
    source: PhoneWhiteSource,
    frame: i64,
}

impl Bench {
    fn started() -> Self {
        Self::started_with(Some(&profile()))
    }

    fn started_with(profile: Option<&DngColorProfile>) -> Self {
        let mut source = PhoneWhiteSource::new(PhoneWhiteConfig::default()).expect("числа по умолчанию верны");
        source.begin_stream(&facts(profile));
        Self { source, frame: 0 }
    }

    /// Показывает кадр и возвращает ответ, если он появился: `(номер кадра, ответ)`.
    fn show(&mut self, hints: CaptureColorHints) -> Option<(i64, WhiteReply)> {
        self.source.observe(&WhiteFrame {
            sensor_timestamp: SensorTimestamp::from_nanos(self.frame * FRAME_NANOS),
            exposure_time: None,
            sensitivity_iso: None,
            color_hints: &hints,
            tiles: None,
            thumbnail: None,
        });
        self.frame += 1;
        self.source.poll().map(|reply| (self.frame - 1, reply))
    }

    fn show_many(&mut self, hints: CaptureColorHints, count: u32) -> Option<(i64, WhiteReply)> {
        (0..count).find_map(|_| self.show(hints))
    }
}

fn facts(profile: Option<&DngColorProfile>) -> WhiteStreamFacts<'_> {
    WhiteStreamFacts {
        camera: CameraTag::of(&CameraId::new("2").expect("имя камеры")),
        profile,
    }
}

fn kelvin_of(reply: &WhiteReply) -> f64 {
    match reply.outcome {
        WhiteSourceOutcome::Ready {
            illuminant: SceneIlluminant::Uniform(white),
            ..
        } => white.temperature.kelvin(),
        other => panic!("ждали готовый ответ, пришло {other:?}"),
    }
}

fn confidence_of(reply: &WhiteReply) -> f32 {
    match reply.outcome {
        WhiteSourceOutcome::Ready { confidence, .. } => confidence.fraction(),
        other => panic!("ждали готовый ответ, пришло {other:?}"),
    }
}

/// Пять кадров прошлого света, затем свежий, чуть дрожащий свет `kelvin`.
fn open_under(bench: &mut Bench, left_over: f64, kelvin: f64, frames: u32) -> Option<(i64, WhiteReply)> {
    if let Some(reply) = bench.show_many(hints_for(left_over), 5) {
        return Some(reply);
    }
    (0..frames).find_map(|index| {
        let jitter = if index % 2 == 0 { 0.5 } else { -0.5 };
        bench.show(hints_for(kelvin + jitter))
    })
}

#[test]
fn opening_under_a_new_light_answers_with_the_new_light_not_with_the_previous_session() {
    let mut bench = Bench::started();
    bench.source.request(WhiteRequestId(1)).expect("заявка принята");
    let (frame, reply) = open_under(&mut bench, 6500.0, 3000.0, 30).expect("ответ есть");
    assert_eq!(reply.request, WhiteRequestId(1));
    assert!((kelvin_of(&reply) - 3000.0).abs() < 25.0, "{reply:?}");
    assert_eq!(frame, 10, "пять кадров прошлого света и шесть ровных свежих");
    assert_eq!(
        reply.measured_frame,
        Some(SensorTimestamp::from_nanos(10 * FRAME_NANOS))
    );
    assert_eq!(confidence_of(&reply), 1.0);
}

#[test]
fn a_recorded_opening_answers_inside_the_hiding_limit() {
    // Запись 220052 (24.11.2026): усиления R/B по кадрам; 0–4 — прошлый сеанс.
    let recorded = [
        (1.873, 2.536),
        (1.873, 2.536),
        (1.873, 2.536),
        (1.873, 2.536),
        (1.873, 2.536),
        (1.899, 2.555),
        (1.981, 2.627),
        (1.980, 2.619),
        (1.930, 2.575),
        (1.939, 2.600),
        (1.927, 2.594),
        (1.930, 2.591),
        (1.918, 2.580),
        (1.926, 2.579),
        (1.927, 2.585),
        (1.922, 2.576),
        (1.909, 2.572),
        (1.912, 2.569),
        (1.906, 2.564),
        (1.901, 2.564),
    ];
    let mut bench = Bench::started();
    bench.source.request(WhiteRequestId(1)).expect("заявка принята");
    let (frame, reply) = recorded
        .iter()
        .find_map(|(red, blue)| bench.show(hints(*red, *blue)))
        .expect("ответ внутри предела");
    assert!((10..=17).contains(&frame), "кадр {frame}");
    let answered = kelvin_of(&reply);
    let direct = white_of_hints(
        &hints(recorded[frame as usize].0, recorded[frame as usize].1),
        &profile(),
    )
    .expect("белая");
    assert!(
        (answered - direct.temperature.0).abs() < 1e-9,
        "ответ — белая этого же кадра"
    );
}

#[test]
fn a_value_that_never_moved_is_not_an_answer_but_dont_know() {
    let mut bench = Bench::started();
    bench.source.request(WhiteRequestId(1)).expect("заявка принята");
    // Запись 160256: за 40 с усиления не сдвинулись ни разу.
    let (frame, reply) = bench.show_many(hints(1.888672, 1.65918), 400).expect("ответ есть");
    assert_eq!(
        reply.outcome,
        WhiteSourceOutcome::Unknown {
            reason: WhiteUnknownReason::NoFreshMeasurement
        }
    );
    assert_eq!(frame, i64::from(PhoneWhiteConfig::default().give_up_frames) - 1);
}

#[test]
fn a_drifting_value_is_given_at_the_deadline_with_lower_confidence() {
    let mut bench = Bench::started();
    bench.source.request(WhiteRequestId(1)).expect("заявка принята");
    let (frame, reply) = (0..60)
        .find_map(|index| bench.show(hints_for(3000.0 + 60.0 * f64::from(index))))
        .expect("ответ есть");
    let config = PhoneWhiteConfig::default();
    assert_eq!(frame, i64::from(config.settle_by_frame) - 1);
    assert_eq!(confidence_of(&reply), config.unsettled_confidence);
}

#[test]
fn dead_automation_and_missing_hints_mean_there_is_no_source() {
    for silent in [hints(1.0, 1.0), CaptureColorHints::default()] {
        let mut bench = Bench::started();
        bench.source.request(WhiteRequestId(1)).expect("заявка принята");
        let (_, reply) = bench.show_many(silent, 60).expect("отказ есть");
        assert_eq!(
            reply.outcome,
            WhiteSourceOutcome::Unavailable {
                reason: WhiteUnavailableReason::AutomationSilent
            }
        );
    }
    assert_eq!(
        white_of_hints(&hints(1.0, 1.0), &profile()),
        Err(HintWhiteError::AutomationSilent)
    );
}

#[test]
fn a_stream_without_colour_profile_refuses_instead_of_guessing() {
    let mut bench = Bench::started_with(None);
    bench.source.request(WhiteRequestId(1)).expect("заявка принята");
    let (_, reply) = open_under(&mut bench, 6500.0, 3000.0, 30).expect("отказ есть");
    assert_eq!(
        reply.outcome,
        WhiteSourceOutcome::Unavailable {
            reason: WhiteUnavailableReason::NoColorProfile
        }
    );
}

#[test]
fn a_request_on_a_running_settled_stream_is_answered_without_new_frames() {
    let mut bench = Bench::started();
    assert_eq!(
        open_under(&mut bench, 6500.0, 4200.0, 40),
        None,
        "без заявки источник молчит"
    );
    bench.source.request(WhiteRequestId(9)).expect("заявка принята");
    let reply = bench.source.poll().expect("ответ сразу");
    assert_eq!(reply.request, WhiteRequestId(9));
    assert!((kelvin_of(&reply) - 4200.0).abs() < 25.0);
    assert_eq!(bench.source.poll(), None, "ответ отдаётся один раз");
}

#[test]
fn a_cancelled_request_is_never_answered() {
    let mut bench = Bench::started();
    bench.source.request(WhiteRequestId(1)).expect("заявка принята");
    bench.source.cancel(WhiteRequestId(1));
    assert_eq!(open_under(&mut bench, 6500.0, 3000.0, 30), None);

    // И готовый, но ещё не забранный ответ.
    bench.source.request(WhiteRequestId(2)).expect("заявка принята");
    bench.source.cancel(WhiteRequestId(2));
    assert_eq!(bench.source.poll(), None);
    // Чужой номер ничего не отменяет.
    bench.source.request(WhiteRequestId(3)).expect("заявка принята");
    bench.source.cancel(WhiteRequestId(2));
    assert!(bench.source.poll().is_some());
}

#[test]
fn a_new_stream_forgets_the_old_light_and_the_old_request() {
    let mut bench = Bench::started();
    open_under(&mut bench, 6500.0, 3000.0, 30);
    bench.source.request(WhiteRequestId(1)).expect("заявка принята");
    let profile = profile();
    bench.source.begin_stream(&facts(Some(&profile)));
    assert_eq!(bench.source.poll(), None, "ответ прежнего потока не отдаётся");
    assert_eq!(open_under(&mut bench, 3000.0, 5200.0, 30), None, "прежняя заявка снята");
    bench.source.request(WhiteRequestId(2)).expect("заявка принята");
    let reply = bench.source.poll().expect("ответ");
    assert!((kelvin_of(&reply) - 5200.0).abs() < 25.0);
}

#[test]
fn neutral_point_is_used_when_gains_are_absent() {
    let profile = profile();
    let white = chromaticity_from_cct_duv(CctDuv {
        temperature: Kelvin(5000.0),
        duv: Duv(0.0),
    })
    .expect("белая");
    let neutral = profile.neutral_for_white(white).expect("нейтраль").values();
    let rational = |value: f64| camera_contract::capabilities::Rational {
        numerator: (value * 1.0e6).round() as i32,
        denominator: 1_000_000,
    };
    let hints = CaptureColorHints {
        neutral_color_point: Some(neutral.map(rational)),
        ..CaptureColorHints::default()
    };
    let measured = white_of_hints(&hints, &profile).expect("белая");
    assert!((measured.temperature.0 - 5000.0).abs() < 40.0, "{measured:?}");
}

#[test]
fn wrong_numbers_are_refused_with_the_field_name() {
    let refuse = |change: fn(&mut PhoneWhiteConfig)| {
        let mut config = PhoneWhiteConfig::default();
        change(&mut config);
        match PhoneWhiteSource::new(config) {
            Err(ConfigError::Invalid { field, .. }) => field,
            Ok(_) => panic!("принят неверный конфиг {config:?}"),
        }
    };
    assert_eq!(refuse(|config| config.stable_frames = 0), "phone.stable_frames");
    assert_eq!(
        refuse(|config| config.stable_relative_tolerance = f64::NAN),
        "phone.stable_relative_tolerance"
    );
    assert_eq!(refuse(|config| config.settle_by_frame = 1), "phone.settle_by_frame");
    assert_eq!(
        refuse(|config| config.unsettled_confidence = 1.5),
        "phone.unsettled_confidence"
    );
    assert_eq!(refuse(|config| config.give_up_frames = 2), "phone.give_up_frames");
}
