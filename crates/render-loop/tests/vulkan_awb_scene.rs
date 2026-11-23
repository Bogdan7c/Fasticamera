//! Наш AWB на настоящем драйвере (RADV, полная validation): синтетическая сцена ColorChecker под лампой 2800 К
//! (камерные значения плиток — из независимой фикстуры `test-assets/scene-estimation`) идёт через боевой путь:
//! импорт → ISP на GPU → статистика AWB → оценка → белая точка → параметры ISP → изображение видоискателя.
//!
//! Доказывается:
//! * холодный старт: первые кадры обработаны, но не показаны; **каждый показанный кадр уже сбалансирован**
//!   (серая плитка нейтральна), хотя без нашего AWB он был бы зелёным (решение 55) или цвета D65;
//! * подсказки HAL (намеренно неверные) на изображение не влияют: два прогона с разными подсказками дают
//!   одинаковые пиксели и одинаковую белую;
//! * кадры серии с брекетингом не показываются, AWB их не использует, камере возвращаются;
//! * «ровно один раз» с новыми счётчиками: обработано = показано + скрыто на холодном старте.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod awb_support;
mod support;
mod vulkan_support;

use std::collections::HashMap;
use std::sync::{Arc, PoisonError};
use std::time::Instant;

use awb_support::{FakeFrame, Returned, SceneImporter, fixture_tiles, mosaic_from_tiles, stream_passport};
use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
use camera_contract::frame_passport::{CaptureColorHints, ColorCorrectionGains, FrameCaptureDetails};
use camera_contract::three_a::{BurstId, DirectiveSlot, RequestOrigin, ThreeAResultState, WhiteBalanceDirective};
use gpu_vulkan::hardware_buffer_cache::ImportCacheCounters;
use gpu_vulkan::sync::GpuWaitFence;
use raw_frame_stream::{OfferedFrame, RawFrameStream, RawFrameStreamConfig, StreamEpoch};
use render_loop::backend::{DisplayBackend, FrameTiming, PresentOutcome, PresentTarget};
use render_loop::config::{
    DisplayColorConfig, RenderLoopConfig, ToneBaseChoice, ViewfinderConfig, ViewfinderToneConfig, WhiteBalanceConfig,
};
use render_loop::vulkan_backend::{ViewfinderReadback, VulkanDisplayBackend};
use shell_contract::clock::MonotonicNanos;
use shell_contract::surface::SurfaceSize;
use support::{Journal, fake_surface};
use vulkan_support::{assert_no_validation_errors, headless_camera_backend_with_directives_or_skip};

const SURFACE_SIZE: SurfaceSize = SurfaceSize {
    width_px: 96,
    height_px: 160,
};
/// Серая плитка сцены: белый образец ColorChecker (ряд 3, столбец 0 → плитки x 0–4, y 18–23), его центр.
const GRAY_PIXEL: (usize, usize) = (2, 20);
/// Допуск нейтральности серой плитки на выходе: 4 шага 10-битного выхода (квантование входа 14 бит и f32 шейдера).
const NEUTRAL_TOLERANCE: f32 = 4.0 / 1023.0;
const FIXTURE_SCENE: &str = "checker_bb2800";
const FIXTURE_LIGHT_KELVIN: f64 = 2800.0;

// ---- стенд ----

const FIRST_TS: i64 = 1_000_000_000;
const FRAME_NANOS: i64 = 16_666_667;
/// Кадры серии: метки далеко от обычных, чтобы импортёр различал их сцену.
const BRACKET_TS_BASE: i64 = 9_000_000_000;

fn timing(frame_index: u64) -> FrameTiming {
    let expected_present = MonotonicNanos::from_nanos(1_000_000 * i64::try_from(frame_index).expect("кадр"));
    FrameTiming {
        frame_index,
        vsync_sequence: frame_index + 1,
        skipped_vsyncs: 0,
        target: PresentTarget::latch_immediately(expected_present, expected_present),
    }
}

struct Rig {
    backend: VulkanDisplayBackend,
    stream: RawFrameStream,
    epoch: StreamEpoch,
    returned: Arc<Returned>,
    hints: CaptureColorHints,
    next_frame: u64,
    next_sequence: i64,
}

/// Результат одного кадра стенда: что показано и как выглядит серая плитка.
struct Shown {
    presented_before: u64,
    presented_after: u64,
    gray: Option<[f32; 3]>,
}

fn rig_or_skip(
    scenes: HashMap<i64, Arc<Vec<u16>>>,
    directives: DirectiveSlot<WhiteBalanceDirective>,
    hints: CaptureColorHints,
    white_balance: WhiteBalanceConfig,
) -> Option<(Rig, Arc<Journal>)> {
    let render_config = RenderLoopConfig {
        display_color: DisplayColorConfig::SrgbOnly,
        viewfinder: ViewfinderConfig {
            white_balance,
            // Стенд проверяет баланс белого, а не тон: цветность плитки меряется на прежней S-кривой, у которой
            // нет сжатия плеча у белого (плечо AgX размывает цвет светлой плитки и занижает разброс каналов).
            // Основы по умолчанию (AgX и др.) проверяют тестами тона и паритета.
            tone: ViewfinderToneConfig {
                base: ToneBaseChoice::Legacy,
                ..ViewfinderToneConfig::default()
            },
            ..ViewfinderConfig::default()
        },
        ..RenderLoopConfig::default()
    };
    // Кадры идут по одному на показ: `LatestWins` выдаёт кадр сразу, без буфера с запасом.
    // Кольцо снимков здесь выключено осознанно: стенд про видоискатель и AWB, а кольцо (по умолчанию 12 кадров,
    // решение 139) держит доли кадров, в том числе кадров серии, и камера получает их назад лишь по мере
    // вытеснения. Тест «кадры серии возвращены камере» без выключения падал бы из-за кольца, а не из-за
    // видоискателя. Поведение кольца со снимками проверяют `vulkan_still_*.rs`.
    let stream = RawFrameStream::new(RawFrameStreamConfig {
        viewfinder_pacing: raw_frame_stream::ViewfinderPacing::LatestWins,
        snapshot_ring_max_frames: 0,
        ..RawFrameStreamConfig::default()
    });
    let mut backend = headless_camera_backend_with_directives_or_skip(
        &render_config,
        stream.viewfinder(),
        Box::new(move |context| {
            Ok(Box::new(SceneImporter {
                context: context.clone(),
                scenes: scenes.clone(),
                entries: HashMap::new(),
                counters: ImportCacheCounters::default(),
            }))
        }),
        directives,
    )?;
    let journal = Arc::new(Journal::default());
    backend
        .attach(fake_surface(&journal, 1), SURFACE_SIZE)
        .expect("surface присоединяется");
    let epoch = stream.producer().begin_stream(Arc::new(stream_passport())).epoch;
    Some((
        Rig {
            backend,
            stream,
            epoch,
            returned: Arc::new(Returned::default()),
            hints,
            next_frame: 0,
            next_sequence: 1,
        },
        journal,
    ))
}

impl Rig {
    /// Предлагает кадр с меткой `sensor_ts` и происхождением `origin`.
    fn offer(&mut self, sensor_ts: i64, origin: RequestOrigin) {
        let producer = self.stream.producer();
        let now = Instant::now();
        producer.offer_frame(
            self.epoch,
            OfferedFrame {
                sensor_timestamp: SensorTimestamp::from_nanos(sensor_ts),
                acquire_fence: GpuWaitFence::already_signaled(),
                frame: Box::new(FakeFrame {
                    sensor_ts,
                    storage: Box::new(0),
                    returned: Arc::clone(&self.returned),
                }),
            },
            now,
        );
        producer.offer_capture_result(
            self.epoch,
            FrameCaptureDetails {
                result: CaptureResultMeta {
                    frame_number: FrameNumber(self.next_sequence),
                    sensor_timestamp: Some(SensorTimestamp::from_nanos(sensor_ts)),
                    exposure_time: None,
                    sensitivity_iso: Some(100),
                    reported_frame_duration: None,
                    rolling_shutter_skew: None,
                    three_a: ThreeAResultState::unreported(origin),
                },
                color: self.hints,
            },
            now,
        );
        self.next_sequence += 1;
    }

    fn presented(&mut self) -> u64 {
        self.backend
            .diagnostics()
            .camera
            .map_or(0, |camera| camera.camera_frames_presented)
    }

    /// Один кадр показа с новым кадром камеры; серая плитка читается, только если кадр показан.
    fn present_new_frame(&mut self, sensor_ts: i64, origin: RequestOrigin) -> Shown {
        self.offer(sensor_ts, origin);
        let presented_before = self.presented();
        assert_eq!(
            self.backend.render_and_present(timing(self.next_frame)),
            PresentOutcome::Presented
        );
        self.next_frame += 1;
        let presented_after = self.presented();
        let gray = (presented_after > presented_before).then(|| self.gray_pixel());
        Shown {
            presented_before,
            presented_after,
            gray,
        }
    }

    /// Кадр показа без нового кадра камеры (перекомпоновка).
    fn tick(&mut self) {
        assert_eq!(
            self.backend.render_and_present(timing(self.next_frame)),
            PresentOutcome::Presented
        );
        self.next_frame += 1;
    }

    fn readback(&self) -> ViewfinderReadback {
        self.backend
            .read_viewfinder_image()
            .expect("readback изображения видоискателя")
            .expect("изображение видоискателя есть")
    }

    fn gray_pixel(&self) -> [f32; 3] {
        let image = self.readback();
        assert_eq!(
            (image.width as usize, image.height as usize),
            (32, 24),
            "ячейка 2×2 → пиксель"
        );
        let pixel = image.pixels[GRAY_PIXEL.1 * image.width as usize + GRAY_PIXEL.0];
        [pixel[0], pixel[1], pixel[2]]
    }

    fn viewfinder(&mut self) -> render_loop::viewfinder::ViewfinderDiagnostics {
        self.backend.diagnostics().viewfinder.expect("диагностика видоискателя")
    }

    fn finish(mut self, journal: &Arc<Journal>) {
        let _ = journal;
        self.stream.producer().end_stream(self.epoch);
        self.tick();
        assert!(self.backend.detach().is_some(), "окно возвращено");
        let counters = self.stream.counters();
        assert_eq!(counters.leaked_leases, 0, "{counters:?}");
        assert_eq!(counters.frames_outstanding, 0, "{counters:?}");
        assert_no_validation_errors(self.backend.context());
    }
}

fn spread(pixel: [f32; 3]) -> f32 {
    let max = pixel.iter().copied().fold(f32::MIN, f32::max);
    let min = pixel.iter().copied().fold(f32::MAX, f32::min);
    max - min
}

fn scene_set(with_bracket: bool) -> HashMap<i64, Arc<Vec<u16>>> {
    let lamp = Arc::new(mosaic_from_tiles(&fixture_tiles(FIXTURE_SCENE)));
    let mut scenes: HashMap<i64, Arc<Vec<u16>>> = (0..60)
        .map(|index| (FIRST_TS + index * FRAME_NANOS, Arc::clone(&lamp)))
        .collect();
    if with_bracket {
        // Кадры серии: насыщенно-синяя сцена, которая испортила бы и картинку, и оценку света.
        let blue = Arc::new(mosaic_from_tiles(&vec![[0.05, 0.1, 0.6]; 768]));
        for index in 0..3 {
            scenes.insert(BRACKET_TS_BASE + index * FRAME_NANOS, Arc::clone(&blue));
        }
    }
    scenes
}

/// Подсказки HAL от другого света: коэффициенты дневного света (D65 ≈ 6500 К) при лампе 2800 К в кадре.
fn wild_hal_hints() -> CaptureColorHints {
    let rational = |numerator| camera_contract::capabilities::Rational {
        numerator,
        denominator: 1000,
    };
    CaptureColorHints {
        neutral_color_point: Some([rational(900), rational(1000), rational(200)]),
        color_gains: Some(ColorCorrectionGains([2.17, 1.0, 1.0, 1.46])),
        ..CaptureColorHints::default()
    }
}

/// Прогон сцены лампы: каждый показанный кадр проверяется на нейтральность серой плитки. Возвращает финальные
/// пиксели изображения и диагностику.
fn run_lamp_scene(hints: CaptureColorHints) -> Option<(Vec<[f32; 4]>, render_loop::viewfinder::ViewfinderDiagnostics)> {
    let (mut rig, journal) = rig_or_skip(
        scene_set(false),
        DirectiveSlot::new(),
        hints,
        WhiteBalanceConfig::default(),
    )?;
    let mut first_presented = None;
    for index in 0..30 {
        let shown = rig.present_new_frame(FIRST_TS + index * FRAME_NANOS, RequestOrigin::Repeating);
        if let Some(gray) = shown.gray {
            first_presented.get_or_insert(index);
            assert!(
                spread(gray) <= NEUTRAL_TOLERANCE,
                "кадр {index}: показанный кадр не сбалансирован, серая плитка {gray:?}"
            );
        }
    }
    let first = first_presented.expect("кадры показываются");
    let diagnostics = rig.viewfinder();
    assert!(
        (1..=4).contains(&first),
        "первые кадры скрыты ({first} — первый показанный): оценка приходит после fence слота"
    );
    let pixels = rig.readback().pixels;
    rig.finish(&journal);
    Some((pixels, diagnostics))
}

#[test]
fn every_presented_frame_is_already_balanced_and_the_cold_start_is_bounded() {
    let Some((pixels, diagnostics)) = run_lamp_scene(CaptureColorHints::default()) else {
        return;
    };
    let counters = &diagnostics.counters;
    let wb = &diagnostics.white_balance;
    // Эталонная проверка чувствительности теста: без нашего AWB (белая D65) серая плитка этой сцены не нейтральна.
    assert!(
        wb.hidden_frames_cold_start >= 1 && wb.hidden_frames_cold_start <= 4,
        "{wb:?}"
    );
    assert_eq!(
        wb.cold_start_forced, 0,
        "оценка пришла сама, принудительного показа не было"
    );
    assert_eq!(
        counters.frames_processed,
        counters.frames_hidden_cold_start + 30 - wb.hidden_frames_cold_start,
        "обработано = показано + скрыто: {counters:?}"
    );
    assert_eq!(counters.frames_hidden_cold_start, wb.hidden_frames_cold_start);
    let scene_white = wb.scene_white.expect("белая сцены оценена");
    assert!(
        (scene_white.temperature_kelvin - FIXTURE_LIGHT_KELVIN).abs() < 80.0,
        "лампа 2800 К распознана: {scene_white:?}"
    );
    assert!(wb.decisions.accepted >= 1 && wb.estimates >= 1, "{wb:?}");
    assert_eq!(wb.table_failure, None);
    // Прореживание: статистика не на каждом кадре, но на холодном старте была на каждом.
    assert!(wb.statistics_skipped_by_decimation > 10, "{wb:?}");
    assert!(wb.statistics_requested > wb.hidden_frames_cold_start, "{wb:?}");
    assert!(!pixels.is_empty());
}

#[test]
fn without_hiding_the_first_frame_would_show_the_provisional_white_and_look_off_colour() {
    // Контроль чувствительности на боевом пути: с отключённым скрытием (`max_hidden_frames = 0`) первый кадр
    // показан с временной белой D65 — серая плитка лампы 2800 К заметно цветная. Значит, нейтральность кадров
    // в основном тесте обеспечивает именно холодный старт, а не случайность сцены.
    let config = WhiteBalanceConfig {
        max_hidden_frames: 0,
        ..WhiteBalanceConfig::default()
    };
    let Some((mut rig, journal)) = rig_or_skip(
        scene_set(false),
        DirectiveSlot::new(),
        CaptureColorHints::default(),
        config,
    ) else {
        return;
    };
    let first = rig.present_new_frame(FIRST_TS, RequestOrigin::Repeating);
    let gray = first.gray.expect("без скрытия первый кадр показан");
    assert!(
        spread(gray) > 0.1,
        "серая плитка под временной белой должна быть цветной: {gray:?}"
    );
    let diagnostics = rig.viewfinder().white_balance;
    assert_eq!(
        (diagnostics.hidden_frames_cold_start, diagnostics.cold_start_forced),
        (0, 1)
    );
    rig.finish(&journal);
}

#[test]
fn wild_hal_colour_hints_change_neither_the_picture_nor_the_white() {
    let Some((quiet_pixels, quiet)) = run_lamp_scene(CaptureColorHints::default()) else {
        return;
    };
    let Some((wild_pixels, wild)) = run_lamp_scene(wild_hal_hints()) else {
        return;
    };
    assert_eq!(
        quiet_pixels, wild_pixels,
        "изображение побитово то же при любых подсказках HAL"
    );
    assert_eq!(quiet.white_balance.scene_white, wild.white_balance.scene_white);
    assert_eq!(quiet.counters.params_resolved, wild.counters.params_resolved);
    assert_eq!(quiet.white_balance.hal_white, None, "у тихого прогона белой HAL нет");
    let hal = wild
        .white_balance
        .hal_white
        .expect("коэффициенты HAL дали белую для сравнения");
    let ours = wild.white_balance.scene_white.expect("наша белая");
    assert!(
        (hal.temperature_kelvin - ours.temperature_kelvin).abs() > 500.0,
        "разница с HAL видна в диагностике: {hal:?} против {ours:?}"
    );
    assert!(wild.white_balance.hal_delta.is_some());
}

#[test]
fn bracket_frames_are_not_shown_not_used_by_awb_and_returned_to_the_camera() {
    let Some((mut rig, journal)) = rig_or_skip(
        scene_set(true),
        DirectiveSlot::new(),
        CaptureColorHints::default(),
        WhiteBalanceConfig::default(),
    ) else {
        return;
    };
    for index in 0..20 {
        rig.present_new_frame(FIRST_TS + index * FRAME_NANOS, RequestOrigin::Repeating);
    }
    let settled = rig.viewfinder();
    let settled_pixels = rig.readback().pixels;
    let settled_white = settled.white_balance.scene_white;
    assert!(settled_white.is_some());

    let burst = BurstId(1);
    for index in 0..3u32 {
        let shown = rig.present_new_frame(
            BRACKET_TS_BASE + i64::from(index) * FRAME_NANOS,
            RequestOrigin::Burst { burst, index },
        );
        assert_eq!(
            shown.presented_after, shown.presented_before,
            "кадр серии {index} не показан"
        );
        assert_eq!(shown.gray, None);
    }
    // Ещё несколько кадров без новых: статистика кадров серии (если бы она считалась) успела бы дойти до AWB.
    for _ in 0..4 {
        rig.tick();
    }
    let after = rig.viewfinder();
    assert_eq!(after.counters.bracket_frames_skipped, 3);
    assert_eq!(
        after.counters.frames_processed, settled.counters.frames_processed,
        "ISP кадры серии не обрабатывал"
    );
    assert_eq!(
        after.white_balance.scene_white, settled_white,
        "AWB кадры серии не использовал"
    );
    assert_eq!(after.white_balance.estimates, settled.white_balance.estimates);
    assert_eq!(rig.readback().pixels, settled_pixels, "изображение осталось прежним");
    let returned = rig.returned.0.lock().unwrap_or_else(PoisonError::into_inner).clone();
    for index in 0..3 {
        assert!(
            returned.contains(&(BRACKET_TS_BASE + index * FRAME_NANOS)),
            "кадр серии {index} возвращён камере"
        );
    }
    // Обычные кадры после серии снова показываются.
    let shown = rig.present_new_frame(FIRST_TS + 20 * FRAME_NANOS, RequestOrigin::Repeating);
    assert!(shown.presented_after > shown.presented_before);
    rig.finish(&journal);
}

#[test]
fn a_manual_directive_applies_at_once_and_hides_nothing() {
    use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
    use camera_contract::three_a::{ThreeAProfile, WhiteBalanceMode};

    let directives = DirectiveSlot::new();
    directives.publish(WhiteBalanceDirective {
        mode: WhiteBalanceMode::Manual {
            setting: WhiteBalanceSetting {
                temperature: ColorTemperature::from_kelvin(FIXTURE_LIGHT_KELVIN).expect("температура"),
                tint: Tint::NEUTRAL,
            },
        },
        profile: ThreeAProfile::Photo,
    });
    let Some((mut rig, journal)) = rig_or_skip(
        scene_set(false),
        directives,
        CaptureColorHints::default(),
        WhiteBalanceConfig::default(),
    ) else {
        return;
    };
    let first = rig.present_new_frame(FIRST_TS, RequestOrigin::Repeating);
    assert_eq!(
        first.presented_after, 1,
        "ручная точка известна сразу: первый кадр показан"
    );
    let gray = first.gray.expect("кадр показан");
    assert!(spread(gray) <= NEUTRAL_TOLERANCE, "{gray:?}");
    let diagnostics = rig.viewfinder();
    assert_eq!(diagnostics.white_balance.hidden_frames_cold_start, 0);
    assert_eq!(diagnostics.white_balance.directive_generation, Some(1));
    rig.finish(&journal);
}
