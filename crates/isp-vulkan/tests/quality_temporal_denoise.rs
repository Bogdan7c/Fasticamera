//! Временной шумодав «на пробу» (`TemporalDenoise`) на реальном GPU: неподвижная шумная сцена
//! действительно становится тише, движущийся край не оставляет «призрака», история сбрасывается при
//! смене потока и по запросу.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod quality_support;

use gpu_vulkan::compute::LocalSize;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::device_buffer::{DeviceBuffer, record_compute_pass_dependency};
use gpu_vulkan::kit_test_support::run_one_shot_blocking;
use isp_vulkan::quality::{
    HistoryState, HistoryUse, LinearRgbPlanes, StreamGeneration, TemporalDenoise, TemporalDenoiseConfig,
};
use quality_support::{NoiseSource, assert_no_validation_errors, extent, gpu_context_or_skip, read_rgb, upload_plane};

const WIDTH: u32 = 64;
const HEIGHT: u32 = 48;
const PIXELS: usize = (WIDTH * HEIGHT) as usize;
/// σ шума кадра в тесте и в модели шума шумодава (шум чтения; дробового нет).
const NOISE_SIGMA: f32 = 0.01;

fn settings() -> TemporalDenoiseConfig {
    TemporalDenoiseConfig {
        enabled: true,
        strength: 0.8,
        motion_threshold_sigmas: 3.0,
        noise_floor: NOISE_SIGMA,
        shot_noise_gain: 0.0,
    }
}

/// Стенд: три плоскости входа + шумодав, привязанный к ним.
struct Rig {
    // Порядок полей: шумодав (его наборы ссылаются на вход) уничтожается раньше входа.
    denoise: TemporalDenoise,
    input: [DeviceBuffer; 3],
}

impl Rig {
    fn new(context: &GpuContext) -> Self {
        let bytes = (PIXELS * 4) as u64;
        let input = [
            DeviceBuffer::new(context, bytes).unwrap(),
            DeviceBuffer::new(context, bytes).unwrap(),
            DeviceBuffer::new(context, bytes).unwrap(),
        ];
        let planes = LinearRgbPlanes {
            red: &input[0],
            green: &input[1],
            blue: &input[2],
        };
        let denoise = TemporalDenoise::new(
            context,
            planes,
            extent(WIDTH, HEIGHT),
            settings(),
            LocalSize::new(16, 16).unwrap(),
            1,
        )
        .unwrap();
        Self { denoise, input }
    }

    /// Подаёт кадр (одинаковые R, G, B) и возвращает выход шумодава (канал R) и что сделано с историей.
    fn frame(&mut self, context: &GpuContext, values: &[f32], stream: StreamGeneration) -> (Vec<f32>, HistoryUse) {
        let (output, history_use) = self.frame_rgb(context, [values, values, values], stream);
        // Каналы обрабатываются одинаково: при одинаковом входе выход одинаков.
        assert!(output.iter().all(|pixel| pixel[0] == pixel[1] && pixel[1] == pixel[2]));
        (output.iter().map(|pixel| pixel[0]).collect(), history_use)
    }

    /// Подаёт кадр с отдельными плоскостями R, G, B; возвращает выход шумодава.
    fn frame_rgb(
        &mut self,
        context: &GpuContext,
        channels: [&[f32]; 3],
        stream: StreamGeneration,
    ) -> (Vec<[f32; 3]>, HistoryUse) {
        for (plane, values) in self.input.iter().zip(channels) {
            upload_plane(plane, values);
        }
        let mut history_use = None;
        run_one_shot_blocking(context, |command_buffer| {
            record_compute_pass_dependency(context, command_buffer);
            history_use = Some(self.denoise.record(command_buffer, 0, stream)?);
            Ok(())
        })
        .unwrap();
        (read_rgb(self.denoise.output()), history_use.unwrap())
    }
}

fn standard_deviation(values: &[f32], mean: f32) -> f32 {
    (values.iter().map(|value| (value - mean).powi(2)).sum::<f32>() / values.len() as f32).sqrt()
}

#[test]
fn static_noisy_scene_gets_quieter_by_measured_factor() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut rig = Rig::new(&context);
    let mut noise = NoiseSource::new(7);
    const CLEAN: f32 = 0.3;
    let stream = StreamGeneration(1);
    let mut last = (Vec::new(), Vec::new());
    for index in 0..30 {
        // Шум каналов независим, как у сенсора (каждый канал — свои фотосайты после демозаика).
        let mut channel = || -> Vec<f32> { (0..PIXELS).map(|_| CLEAN + NOISE_SIGMA * noise.gaussian()).collect() };
        let (red, green, blue) = (channel(), channel(), channel());
        let (output, history_use) = rig.frame_rgb(&context, [&red, &green, &blue], stream);
        let expected = if index == 0 {
            HistoryUse::Restarted
        } else {
            HistoryUse::Blended
        };
        assert_eq!(history_use, expected, "кадр {index}");
        last = (
            [red, green, blue].concat(),
            [0, 1, 2]
                .iter()
                .flat_map(|c| output.iter().map(move |pixel| pixel[*c]))
                .collect(),
        );
    }
    let input_noise = standard_deviation(&last.0, CLEAN);
    let output_noise = standard_deviation(&last.1, CLEAN);
    let factor = input_noise / output_noise;
    println!("шум неподвижной сцены: вход σ {input_noise:.5}, выход σ {output_noise:.5}, в {factor:.2} раза тише");
    // Теория для чистой рекурсии с весом 0,8: σ втрое меньше; адаптация к движению часть пикселей
    // отпускает (шумовые разности до T·σ), поэтому требуем не меньше 2×.
    assert!(factor >= 2.0, "шумодав подавил шум лишь в {factor:.2} раза");
    // Среднее не смещено (рекурсия с весом < 1 не «тянет» уровень).
    let mean = last.1.iter().sum::<f32>() / last.1.len() as f32;
    assert!((mean - CLEAN).abs() < 1e-3, "среднее сместилось: {mean}");
    assert_no_validation_errors(&context);
}

/// Кадр с вертикальным краем: левее `edge_x` — тёмный, правее — светлый (без шума).
fn edge_frame(edge_x: u32) -> Vec<f32> {
    (0..PIXELS)
        .map(|index| if (index as u32 % WIDTH) < edge_x { 0.1 } else { 0.6 })
        .collect()
}

#[test]
fn moving_edge_leaves_no_ghost_beyond_bound() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut rig = Rig::new(&context);
    let stream = StreamGeneration(1);
    let mut worst_ghost = 0.0_f32;
    for step in 0..8 {
        let current = edge_frame(8 + 4 * step);
        let (output, _) = rig.frame(&context, &current, stream);
        let ghost = output
            .iter()
            .zip(&current)
            .map(|(got, want)| (got - want).abs())
            .fold(0.0_f32, f32::max);
        worst_ghost = worst_ghost.max(ghost);
    }
    println!("движущийся край: наибольший след прошлого кадра {worst_ghost:e} при скачке 0,5");
    // Скачок 0,5 = 50σ ≫ порога 3σ: вес истории ровно 0, следа быть не должно вовсе. Граница 1 % скачка
    // оставлена на округление.
    assert!(worst_ghost <= 0.005, "призрак {worst_ghost}");
    assert_no_validation_errors(&context);
}

#[test]
fn history_restarts_on_stream_change_and_on_request() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut rig = Rig::new(&context);
    assert_eq!(rig.denoise.history_state(), HistoryState::Empty);
    let first = vec![0.30_f32; PIXELS];
    // Небольшой сдвиг (1σ) — без сброса он смешался бы с историей.
    let shifted = vec![0.31_f32; PIXELS];
    rig.frame(&context, &first, StreamGeneration(1));
    let (blended, history_use) = rig.frame(&context, &shifted, StreamGeneration(1));
    assert_eq!(history_use, HistoryUse::Blended);
    assert!(
        blended.iter().all(|value| *value < 0.309),
        "тот же поток — смешивание с историей"
    );

    let (restarted, history_use) = rig.frame(&context, &shifted, StreamGeneration(2));
    assert_eq!(history_use, HistoryUse::Restarted);
    assert_eq!(
        restarted, shifted,
        "новый поток — история = текущий кадр, без следа прошлого потока"
    );
    assert_eq!(
        rig.denoise.history_state(),
        HistoryState::Accumulating {
            stream: StreamGeneration(2)
        }
    );

    rig.denoise.reset_history();
    assert_eq!(rig.denoise.history_state(), HistoryState::Empty);
    let (after_reset, history_use) = rig.frame(&context, &first, StreamGeneration(2));
    assert_eq!(history_use, HistoryUse::Restarted);
    assert_eq!(after_reset, first, "сброс по запросу — история = текущий кадр");
    assert_no_validation_errors(&context);
}

#[test]
fn invalid_settings_and_mismatched_input_are_rejected_before_gpu_objects() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let small = DeviceBuffer::new(&context, 16).unwrap();
    let planes = LinearRgbPlanes {
        red: &small,
        green: &small,
        blue: &small,
    };
    let local = LocalSize::new(8, 8).unwrap();
    assert!(
        TemporalDenoise::new(&context, planes, extent(WIDTH, HEIGHT), settings(), local, 1).is_err(),
        "размер"
    );
    let broken = TemporalDenoiseConfig {
        strength: 1.5,
        ..settings()
    };
    assert!(
        TemporalDenoise::new(&context, planes, extent(2, 2), broken, local, 1).is_err(),
        "конфиг"
    );
    drop(small);
    assert_eq!(context.live_objects(), baseline);
}
