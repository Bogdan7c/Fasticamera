//! Замкнутая симуляция: «сенсор» с задержкой применения экспозиции, статистика каждый 4-й кадр,
//! настоящие `CpuExposureMeter` и `AutoExposureController`. Проверяем то, что видит пользователь:
//! сходимость к среднему серому без перерегулирования, отсутствие «дыхания» в видео, блокировку и
//! остаточную подтяжку во время догоняния сенсора.
//!
//! Модель: сцена — равномерно серая, яркость кадра `Y = min(S · A, 1)`, где `A` — произведение
//! «выдержка × ISO» этого кадра, `S` — «яркость сцены на единицу произведения». Идеальное
//! произведение — то, при котором `Y` равна среднему серому тона: `P_ideal = 0,075 / S`.

#![allow(clippy::unwrap_used, reason = "в тестах падение — сигнал провала")]

use camera_contract::auto_exposure::{AppliedExposure, MeteredArea};
use camera_contract::capture_result::SensorTimestamp;
use camera_contract::three_a::{AeAfLock, ThreeAProfile};
use isp_model::params::ExposureGain;

use super::config::{ExposureControllerConfig, MeteringConfig};
use super::controller::AutoExposureController;
use super::meter::{CpuExposureMeter, ExposureMeter, ExposureMeteringRequest};
use super::metering_weights::{MeteringArea, MeteringWeights, TilePlacement};
use super::test_support::{GRID, plain_luminance, product, stats_from, target};
use crate::executor::FrameTag;
use camera_contract::auto_exposure::ExposureDirective;

/// Кадров между замерами статистики.
const STATS_EVERY: i64 = 4;
/// Задержка применения экспозиции сенсором, кадров (замер 00: 7–8).
const SENSOR_DELAY: usize = 8;
const FRAME_NANOS: i64 = 16_666_667;
const MIDDLE_GRAY: f64 = 0.075;

/// Детерминированный шум замера в диапазоне ±amplitude EV.
struct Noise {
    state: u64,
    amplitude_ev: f64,
}

impl Noise {
    fn next_ev(&mut self) -> f64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let unit = (self.state >> 11) as f64 / (1u64 << 53) as f64;
        (unit * 2.0 - 1.0) * self.amplitude_ev
    }
}

/// Что видно по каждому кадру.
#[derive(Debug, Clone, Copy)]
struct FrameLog {
    /// Произведение, реально стоявшее на кадре.
    applied: f64,
    /// Остаточная подтяжка ISP на кадре.
    gain: ExposureGain,
}

struct Rig {
    controller: AutoExposureController,
    meter: CpuExposureMeter,
    weights: MeteringWeights,
    directive: ExposureDirective,
    scene_gain: f64,
    command: f64,
    history: Vec<f64>,
    frame: i64,
    noise: Option<Noise>,
    /// Столбцы плиток, где свет далеко за белым при любой экспозиции (окно контровой сцены).
    window_columns: Option<std::ops::Range<u32>>,
    log: Vec<FrameLog>,
    proposals: u32,
    /// Цель каждой опубликованной команды (для проверки «не дышит»).
    commands_seen: Vec<f64>,
}

impl Rig {
    /// Старт на кадре 0: произведение `initial`, сцена с идеалом `ideal_at_start`.
    fn start(directive: ExposureDirective, initial: f64, ideal_at_start: f64) -> Self {
        let mut controller = AutoExposureController::new(ExposureControllerConfig::default()).unwrap();
        let directive = ExposureDirective {
            applied: Some(AppliedExposure {
                product: product(initial),
                shortfall_ev: 0.0,
            }),
            ..directive
        };
        controller.set_directive(directive);
        controller.begin_stream();
        let weights = MeteringWeights::build(
            GRID,
            TilePlacement::full_frame(4.0 / 3.0).unwrap(),
            MeteringArea::Matrix,
            &MeteringConfig::default(),
        )
        .unwrap();
        Self {
            controller,
            meter: CpuExposureMeter::new(MeteringConfig::default()).unwrap(),
            weights,
            directive,
            scene_gain: MIDDLE_GRAY / ideal_at_start,
            command: initial,
            history: Vec::new(),
            frame: 0,
            noise: None,
            window_columns: None,
            log: Vec::new(),
            proposals: 0,
            commands_seen: Vec::new(),
        }
    }

    fn ideal(&self) -> f64 {
        MIDDLE_GRAY / self.scene_gain
    }

    /// Сцена стала ярче (положительное `ev`) или темнее.
    fn change_scene_by(&mut self, ev: f64) {
        self.scene_gain *= ev.exp2();
    }

    fn set_directive(&mut self, directive: ExposureDirective) {
        self.directive = ExposureDirective {
            applied: self.directive.applied,
            ..directive
        };
        self.controller.set_directive(self.directive);
    }

    /// Произведение, стоявшее на кадре `index`: команда, выданная `SENSOR_DELAY` кадров назад.
    fn applied_at(&self, index: usize) -> f64 {
        match index.checked_sub(SENSOR_DELAY) {
            Some(commanded_at) => self.history[commanded_at],
            None => self.history.first().copied().unwrap_or(self.command),
        }
    }

    fn run(&mut self, frames: i64) {
        for _ in 0..frames {
            self.step();
        }
    }

    fn step(&mut self) {
        let index = self.frame;
        let applied = self.applied_at(index as usize);
        let timestamp = SensorTimestamp::from_nanos(index * FRAME_NANOS);
        if index % STATS_EVERY == 0 {
            self.measure(index, applied);
        }
        self.controller.advance(timestamp);
        if let Some(proposal) = self.controller.proposal(timestamp) {
            self.proposals += 1;
            self.command = proposal.target.seconds_iso();
            self.commands_seen.push(self.command);
            self.directive.applied = Some(AppliedExposure {
                product: proposal.target,
                shortfall_ev: 0.0,
            });
            self.controller.set_directive(self.directive);
        }
        self.history.push(self.command);
        let gain = self.controller.display_gain(Some(product(applied)));
        self.log.push(FrameLog { applied, gain });
        self.frame += 1;
    }

    /// Статистика кадра по яркости `Y = min(S·A, 1)` (с шумом замера, если задан) → замер → контроллер.
    fn measure(&mut self, index: i64, applied: f64) {
        let noise_ev = self.noise.as_mut().map_or(0.0, Noise::next_ev);
        let luminance = (self.scene_gain * applied * noise_ev.exp2()).min(1.0) as f32;
        let window = self.window_columns.clone();
        let stats = stats_from(|column, _| match &window {
            Some(columns) if columns.contains(&column) => [1.0; 3],
            _ => [luminance; 3],
        });
        self.meter
            .submit(ExposureMeteringRequest {
                frame: FrameTag {
                    stream_generation: 1,
                    sequence: index as u64,
                    sensor_timestamp_nanos: index * FRAME_NANOS,
                },
                area: MeteredArea::WholeFrame,
                statistics: &stats,
                weights: &self.weights,
                luminance: plain_luminance(),
                frame_product: product(applied),
                target: target(),
            })
            .unwrap();
        while let Some(done) = self.meter.poll() {
            self.controller.ingest(&done);
        }
    }

    /// Ошибка кадра `index` относительно идеала сцены, EV (>0 — кадр светлее нужного).
    fn error_ev(&self, index: usize) -> f64 {
        (self.log[index].applied / self.ideal()).log2()
    }

    /// Ошибка уровня на экране после подтяжки, EV: `log2(min(S·A·k, 1) / 0,075)`.
    fn screen_error_ev(&self, index: usize) -> f64 {
        let entry = self.log[index];
        let level = (self.scene_gain * entry.applied * f64::from(entry.gain.multiplier())).min(1.0);
        (level / MIDDLE_GRAY).log2()
    }
}

fn settled_rig(directive: ExposureDirective) -> Rig {
    let mut rig = Rig::start(directive, 0.01, 0.01);
    rig.run(60);
    assert!(rig.error_ev(59).abs() < 0.05, "старт на идеале должен быть спокоен");
    rig
}

fn video() -> ExposureDirective {
    ExposureDirective {
        profile: ThreeAProfile::Video,
        ..ExposureDirective::DEFAULT
    }
}

/// Кадры на сходимость фото: 5τ (45 кадров) + задержка сенсора + период статистики.
const PHOTO_CONVERGENCE_FRAMES: usize = 45 + SENSOR_DELAY + STATS_EVERY as usize;

#[test]
fn photo_step_of_plus_and_minus_three_stops_converges_without_overshoot() {
    for step_ev in [3.0_f64, -3.0] {
        let mut rig = settled_rig(ExposureDirective::DEFAULT);
        let step_frame = rig.frame as usize;
        rig.change_scene_by(step_ev);
        rig.run(PHOTO_CONVERGENCE_FRAMES as i64 + 30);
        // Кадр на момент ступеньки старый: сцена ярче на 3 EV — он светлее нужного на те же 3 EV.
        let initial_error = rig.error_ev(step_frame);
        assert!(
            (initial_error - step_ev).abs() < 0.2,
            "ступенька {step_ev}: ошибка {initial_error}"
        );
        let tolerance = f64::from(ExposureControllerConfig::default().settled_tolerance_ev);
        let settled_by = step_frame + PHOTO_CONVERGENCE_FRAMES + SENSOR_DELAY;
        assert!(
            rig.error_ev(settled_by).abs() <= tolerance,
            "ступенька {step_ev}: на кадре {settled_by} ошибка {}",
            rig.error_ev(settled_by)
        );
        // Без перерегулирования: ошибка не меняет знак больше чем на допуск.
        for index in step_frame..rig.log.len() {
            assert!(
                rig.error_ev(index) * step_ev.signum() >= -tolerance,
                "ступенька {step_ev}: перерегулирование на кадре {index}: {}",
                rig.error_ev(index)
            );
        }
    }
}

#[test]
fn start_with_blown_highlights_and_with_darkness_both_converge() {
    // +4 EV пересвета: весь кадр выше порога, точной оценки нет — идёт ограниченный шаг.
    let mut bright = Rig::start(ExposureDirective::DEFAULT, 0.01 * 16.0, 0.01);
    bright.run(180);
    assert!(bright.log[0].applied > 0.1, "старт действительно в пересвете");
    assert!(bright.error_ev(179).abs() <= 0.1, "пересвет: {}", bright.error_ev(179));
    // −6 EV темноты: кадр ниже порога тёмных плиток.
    let mut dark = Rig::start(ExposureDirective::DEFAULT, 0.01 / 64.0, 0.01);
    dark.run(180);
    assert!(dark.error_ev(179).abs() <= 0.1, "темнота: {}", dark.error_ev(179));
    // И оба прошли через ограниченные шаги, а не угадали с первого замера.
    assert!(bright.proposals > 1 && dark.proposals > 1);
}

#[test]
fn video_does_not_breathe_on_measurement_noise_but_photo_stays_within_tolerance() {
    let mut video_rig = settled_rig(video());
    let calm_commands = video_rig.commands_seen.len();
    video_rig.noise = Some(Noise {
        state: 12345,
        amplitude_ev: 0.1,
    });
    video_rig.run(900);
    let settled_target = video_rig.commands_seen[calm_commands - 1];
    assert!(
        video_rig.commands_seen[calm_commands..]
            .iter()
            .all(|command| (command.log2() - settled_target.log2()).abs() < 0.02),
        "видео дышит: {:?}",
        &video_rig.commands_seen[calm_commands..]
    );
    let video_swing = (60..900).map(|i| video_rig.error_ev(i).abs()).fold(0.0_f64, f64::max);
    assert!(video_swing < 0.12, "видео ушло от идеала на {video_swing} EV");

    let mut photo_rig = settled_rig(ExposureDirective::DEFAULT);
    photo_rig.noise = Some(Noise {
        state: 12345,
        amplitude_ev: 0.1,
    });
    photo_rig.run(900);
    let swing = (60..900).map(|i| photo_rig.error_ev(i).abs()).fold(0.0_f64, f64::max);
    assert!(swing <= 0.11, "фото вышло за допуск: {swing}");
    let (low, high) = photo_rig.log[60..]
        .iter()
        .map(|entry| entry.applied.log2())
        .fold((f64::MAX, f64::MIN), |(low, high), value| {
            (low.min(value), high.max(value))
        });
    assert!(high - low > 0.02, "фото должно следить за шумом, размах {}", high - low);
}

#[test]
fn locked_exposure_ignores_scene_changes_until_unlocked() {
    let mut rig = settled_rig(ExposureDirective::DEFAULT);
    rig.set_directive(ExposureDirective {
        lock: AeAfLock::Locked,
        ..ExposureDirective::DEFAULT
    });
    rig.run(10);
    assert!(rig.controller.is_holding());
    let frozen = rig.command;
    rig.change_scene_by(2.0);
    rig.run(180);
    assert_eq!(rig.command, frozen, "блокировка держит экспозицию при смене сцены");
    assert!(
        (rig.error_ev(rig.log.len() - 1) - 2.0).abs() < 0.1,
        "кадр остался светлее нужного на 2 EV"
    );
    rig.set_directive(ExposureDirective::DEFAULT);
    rig.run(PHOTO_CONVERGENCE_FRAMES as i64 + 60);
    assert!(
        rig.error_ev(rig.log.len() - 1).abs() <= 0.1,
        "после снятия блокировки сошлись"
    );
}

#[test]
fn residual_gain_keeps_the_screen_closer_to_middle_gray_during_a_step() {
    let mut with_gain = settled_rig(ExposureDirective::DEFAULT);
    let step_frame = with_gain.frame as usize;
    with_gain.change_scene_by(1.5);
    with_gain.run(90);
    let range = step_frame..with_gain.log.len();
    // Подтяжка всегда в пределах ±0,5 EV.
    assert!(
        range.clone().all(|i| with_gain.log[i].gain.ev().abs() <= 0.5 + 1e-6),
        "подтяжка вышла за предел"
    );
    assert!(
        range.clone().any(|i| with_gain.log[i].gain.ev().abs() > 0.3),
        "во время ступеньки подтяжка должна работать"
    );
    let screen_error: f64 = range.clone().map(|i| with_gain.screen_error_ev(i).abs()).sum();
    let raw_error: f64 = range.clone().map(|i| with_gain.error_ev(i).abs()).sum();
    assert!(
        screen_error < 0.9 * raw_error,
        "с подтяжкой {screen_error}, без {raw_error}"
    );
    // Худший кадр тоже лучше, а не только сумма.
    let worst_screen = range
        .clone()
        .map(|i| with_gain.screen_error_ev(i).abs())
        .fold(0.0_f64, f64::max);
    let worst_raw = range.map(|i| with_gain.error_ev(i).abs()).fold(0.0_f64, f64::max);
    assert!(worst_screen < worst_raw, "{worst_screen} против {worst_raw}");
}

#[test]
fn backlit_scene_with_a_window_far_beyond_white_settles_without_a_limit_cycle() {
    // Окно в центре (~40 % веса матрицы) всегда пересвечено, остальное тёмное: доля пересвета у
    // границы `clipped_fraction_limit`, где раньше экспозиция ходила вверх-вниз.
    for window in [10..22, 11..21, 12..20] {
        let mut rig = Rig::start(ExposureDirective::DEFAULT, 0.01, 0.01);
        rig.window_columns = Some(window.clone());
        rig.run(600);
        let tail = &rig.log[420..];
        let (low, high) = tail
            .iter()
            .map(|entry| entry.applied.log2())
            .fold((f64::MAX, f64::MIN), |(low, high), value| {
                (low.min(value), high.max(value))
            });
        let tolerance = f64::from(ExposureControllerConfig::default().settled_tolerance_ev);
        assert!(
            high - low <= tolerance,
            "окно {window:?}: экспозиция гуляет на {} EV",
            high - low
        );
    }
}
