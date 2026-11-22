//! Кадр телефона 4096×3072: захват и проявление по полосам на ПК (RADV). Цифры времени — ТОЛЬКО информация
//! (печатаются, порогов нет: на Adreno DVFS растягивает работу, мс там не цена); тест проверяет другое — что
//! на кадре настоящего размера (индексы, смещения полос, память) результат верен: середина кадра, через границу
//! полос, совпадает с эталоном `isp-model` на подкадре (≤ 1 LSB), а картинка не вырождена.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod photo_support;
mod quality_support;

use std::time::{Duration, Instant};

use camera_contract::geometry::PixelSize;
use gpu_vulkan::kit_test_support::run_one_shot_blocking_traced;
use isp_model::cfa::BayerPattern;
use isp_model::image::RawMosaic;
use isp_model::pipeline::run_reference;
use isp_vulkan::photo::{PhotoConfig, RawCapturer, RgbaReadback};
use photo_support::{
    Placement, SourceFrame, SourceKind, crop_in_buffer, develop, developer_for, into_gpu_error, real_frame,
    reference_rgba, upload,
};
use quality_support::{NoiseSource, assert_no_validation_errors, gpu_context_or_skip};

const WIDTH: u32 = 4096;
const HEIGHT: u32 = 3072;
const SLICE_ROWS: u32 = 256;

/// Кадр 4096×3072: гладкий градиент с «текстурой» (шум) — реальные уровни калибровки OnePlus 11 из кропа.
fn phone_frame() -> SourceFrame {
    let template = real_frame("real-main-lamp-gray-tile");
    let pattern = template.raw.pattern();
    let mut noise = NoiseSource::new(11);
    let samples: Vec<u16> = (0..HEIGHT)
        .flat_map(|y| (0..WIDTH).map(move |x| (x, y)))
        .map(|(x, y)| {
            let gradient = 0.15 + 0.5 * (f64::from(x) / f64::from(WIDTH)) + 0.2 * (f64::from(y) / f64::from(HEIGHT));
            let texture = if (x / 64 + y / 64) % 2 == 0 { 0.08 } else { -0.08 };
            let grain = 0.01 * f64::from(noise.gaussian());
            (64.0 + (gradient + texture + grain).clamp(0.0, 1.0) * 959.0).round() as u16
        })
        .collect();
    SourceFrame {
        name: "phone_4096x3072".to_owned(),
        raw: RawMosaic::new(PixelSize::new(WIDTH, HEIGHT), pattern, samples).unwrap(),
        params: template.params,
        sensor: template.sensor,
        neutral: template.neutral,
    }
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

#[test]
fn phone_frame_in_slices_is_correct_and_timings_are_reported() {
    let Some(context) = gpu_context_or_skip() else { return };
    let frame = phone_frame();
    assert_eq!(
        frame.raw.pattern(),
        BayerPattern::BGGR,
        "раскладка калибровки OnePlus 11"
    );
    let placement = Placement {
        left: 8,
        top: 8,
        right_padding: 8,
        bottom_padding: 8,
        padding_value: 1023,
    };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let mut captured_for_development = None;
    for kind in [SourceKind::Raw16, SourceKind::Raw10] {
        let gpu_frame = upload(&context, &frame.raw, kind, placement);
        // Захват: одна разовая отправка с ожиданием fence (на ПК ≈ мс GPU вместе с отправкой).
        let mut pending = None;
        let began = Instant::now();
        let run = run_one_shot_blocking_traced(&context, |command_buffer| {
            pending = Some(
                capturer
                    .record_capture(command_buffer, &gpu_frame.source, gpu_frame.valid_area)
                    .map_err(into_gpu_error)?,
            );
            Ok(())
        })
        .unwrap();
        let capture_time = began.elapsed();
        let mut pending = pending.unwrap();
        pending.note_submitted(&run.work);
        let captured = capturer.complete_after_fence(pending, &run.proof).unwrap();
        println!(
            "захват 4096×3072 {kind:?}: {:.2} мс (отправка + fence на ПК)",
            millis(capture_time)
        );
        let samples = captured.samples().unwrap();
        assert_eq!(
            samples.sample(WIDTH - 1, HEIGHT - 1),
            Some(frame.raw.sample(WIDTH - 1, HEIGHT - 1))
        );
        assert_eq!(samples.sample(1234, 2345), Some(frame.raw.sample(1234, 2345)));
        if kind == SourceKind::Raw10 {
            captured_for_development = Some((captured, gpu_frame));
        }
    }

    let (captured, gpu_frame) = captured_for_development.unwrap();
    let mut developer = developer_for(&context, SLICE_ROWS, WIDTH);
    let crop = crop_in_buffer(gpu_frame.valid_area, 0, 0, WIDTH, HEIGHT);
    let mut developed = develop(
        &context,
        &mut developer,
        &captured,
        &frame,
        crop,
        RgbaReadback::NotNeeded,
    );
    // Миниатюра кадра 12 Мп (ненаправленное уменьшение ≈ 16:1, дробей нет) равна эталону от полной картинки.
    let began = Instant::now();
    let isp_vulkan::photo::ThumbnailOutcome::Ready { size, rgba } = developed.photo.take_thumbnail() else {
        panic!("миниатюра кадра 12 Мп обязана быть");
    };
    let (want_size, want) = isp_vulkan::photo::reference_thumbnail(
        &developed.image_rgba,
        PixelSize::new(WIDTH, HEIGHT),
        still_contract::shot::StillRotation::None,
        PhotoConfig::default().thumbnail_long_side_px,
    )
    .unwrap();
    assert_eq!((size, want_size), (PixelSize::new(256, 192), PixelSize::new(256, 192)));
    let worst = rgba
        .iter()
        .zip(&want)
        .map(|(got, want)| got.abs_diff(*want))
        .max()
        .unwrap();
    assert!(worst <= 1, "миниатюра 12 Мп против эталона: {worst} градаций");
    println!(
        "эталон миниатюры 12 Мп на CPU посчитан за {:.0} мс",
        millis(began.elapsed())
    );
    let total: Duration = developed.slice_times.iter().sum();
    let slowest = developed.slice_times.iter().max().copied().unwrap();
    println!(
        "проявление 4096×3072, полоса {SLICE_ROWS} строк: {} полос, всего {:.1} мс, одна полоса {:.1}..{:.1} мс (в среднем {:.1}); \
         плоскости {:.1} МиБ",
        developed.slice_times.len(),
        millis(total),
        millis(developed.slice_times.iter().min().copied().unwrap()),
        millis(slowest),
        millis(total) / developed.slice_times.len() as f64,
        developer.plane_memory_bytes() as f64 / (1 << 20) as f64,
    );
    // 12 полос вывода по 256 строк + шаг отделки (миниатюра, переход, копии; 14g).
    assert_eq!(developed.slice_times.len(), 13);
    assert_eq!(developed.image_rgba.len(), (WIDTH * HEIGHT * 4) as usize);

    // Середина кадра через границу полос (строка 1536 = 6 × 256) против эталона на подкадре 256×256: пиксели
    // дальше ореола от краёв подкадра обязаны совпасть (до 1 LSB).
    let (sub_left, sub_top, side) = (1000_u32, 1400_u32, 256_u32);
    let sub_samples: Vec<u16> = (sub_top..sub_top + side)
        .flat_map(|y| (sub_left..sub_left + side).map(move |x| (x, y)))
        .map(|(x, y)| frame.raw.sample(x, y))
        .collect();
    let sub_raw = RawMosaic::new(PixelSize::new(side, side), frame.raw.pattern(), sub_samples).unwrap();
    let reference = reference_rgba(&run_reference(&sub_raw, &frame.params).unwrap());
    let margin = 16;
    let mut worst = 0_u8;
    let mut compared = 0_u32;
    for y in margin..side - margin {
        for x in margin..side - margin {
            let got_at = (((sub_top + y) * WIDTH + sub_left + x) * 4) as usize;
            let want_at = ((y * side + x) * 4) as usize;
            for channel in 0..4 {
                worst = worst.max(developed.image_rgba[got_at + channel].abs_diff(reference[want_at + channel]));
            }
            compared += 1;
        }
    }
    assert!(compared > 50_000);
    assert!(
        worst <= 1,
        "середина кадра 4096×3072 против эталона: max |Δ| {worst} LSB"
    );

    // Картинка не вырождена: градиент и текстура дают большой размах.
    let values = developed.image_rgba.chunks_exact(4).map(|pixel| pixel[1]);
    let (darkest, brightest) = values.fold((255_u8, 0_u8), |(low, high), value| (low.min(value), high.max(value)));
    assert!(brightest - darkest > 80, "размах зелёного {darkest}..{brightest}");
    assert_no_validation_errors(&context);
}

/// Те же стадии на контексте БЕЗ validation: слой validation замедляет каждый вызов, и цифры первого теста
/// завышены; здесь — ближе к чистому времени GPU (отправка + fence на ПК). Тоже только информация.
#[test]
fn phone_frame_timings_without_validation_layer() {
    use gpu_vulkan::context::{GpuContext, GpuContextConfig};
    use gpu_vulkan::device::DeviceProfile;
    use gpu_vulkan::validation::ValidationMode;
    let config = GpuContextConfig::new(DeviceProfile::Headless, ValidationMode::Disabled);
    let context = match GpuContext::new(&config) {
        Ok(context) => context,
        // Политика GPU-тестов крейта: нет GPU — провал, если не задан явный пропуск.
        Err(error) if std::env::var("FASTICAMERA_SKIP_GPU_TESTS").as_deref() == Ok("1") => {
            eprintln!("ПРОПУСК GPU-теста: {error} (FASTICAMERA_SKIP_GPU_TESTS=1)");
            return;
        }
        Err(error) => {
            panic!("GPU недоступен: {error}. Установите FASTICAMERA_SKIP_GPU_TESTS=1, чтобы пропустить осознанно")
        }
    };
    let frame = phone_frame();
    let placement = Placement {
        left: 8,
        top: 8,
        right_padding: 8,
        bottom_padding: 8,
        padding_value: 1023,
    };
    let gpu_frame = upload(&context, &frame.raw, SourceKind::Raw10, placement);
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let valid_size = PixelSize::new(WIDTH, HEIGHT);
    // Два варианта: буфер выделяется внутри `record_capture` (медленный путь) и заранее через `prepare`.
    let mut captured = None;
    for prepared in [false, true] {
        let mut total_times = Vec::new();
        let mut record_times = Vec::new();
        for _ in 0..5 {
            if prepared {
                capturer.prepare(valid_size).unwrap();
            }
            let mut pending = None;
            let mut record_time = Duration::ZERO;
            let began = Instant::now();
            let run = run_one_shot_blocking_traced(&context, |command_buffer| {
                let record_began = Instant::now();
                pending = Some(
                    capturer
                        .record_capture(command_buffer, &gpu_frame.source, gpu_frame.valid_area)
                        .map_err(into_gpu_error)?,
                );
                record_time = record_began.elapsed();
                Ok(())
            })
            .unwrap();
            total_times.push(began.elapsed());
            record_times.push(record_time);
            let mut pending = pending.unwrap();
            pending.note_submitted(&run.work);
            captured = Some(capturer.complete_after_fence(pending, &run.proof).unwrap());
        }
        total_times.sort();
        record_times.sort();
        println!(
            "без validation: захват RAW10 4096×3072, буфер {}: запись `record_capture` на CPU (поток рендера) \
             {:.2} мс; всего запись + отправка + ожидание fence {:.2} мс (медианы из 5, стенное время, не метки GPU)",
            if prepared {
                "ЗАРАНЕЕ"
            } else {
                "внутри record_capture"
            },
            millis(record_times[2]),
            millis(total_times[2])
        );
    }
    let captured = captured.unwrap();
    let mut first_image: Option<Vec<u8>> = None;
    for slice_rows in [128, 256, 512] {
        let mut developer = developer_for(&context, slice_rows, WIDTH);
        let crop = crop_in_buffer(gpu_frame.valid_area, 0, 0, WIDTH, HEIGHT);
        let mut totals = Vec::new();
        let mut slowest = Duration::ZERO;
        let mut slices = 0;
        let (mut first_slice_times, mut last_slice_times) = (Vec::new(), Vec::new());
        // Начало проявки на CPU (картинка 48 МиБ, uniform, привязки; для JPEG ещё буфер 48 МиБ с обнулением).
        for readback in [RgbaReadback::NotNeeded, RgbaReadback::ForJpeg] {
            let began = Instant::now();
            developer
                .begin(&isp_vulkan::photo::DevelopRequest {
                    raw: &captured,
                    params: &frame.params,
                    crop,
                    readback,
                    rotation: still_contract::shot::StillRotation::None,
                })
                .unwrap();
            let begin_time = began.elapsed();
            developer.cancel().unwrap();
            println!(
                "без validation: begin() на CPU, полоса {slice_rows}, {readback:?}: {:.2} мс",
                millis(begin_time)
            );
        }
        for _ in 0..3 {
            let developed = develop(
                &context,
                &mut developer,
                &captured,
                &frame,
                crop,
                RgbaReadback::NotNeeded,
            );
            totals.push(developed.slice_times.iter().sum::<Duration>());
            // Первая и последняя полосы одной геометрии (ореол с одной стороны); миниатюра — только в последней.
            // Последний шаг — отделка; последняя полоса вывода — перед ним (той же геометрии, что первая).
            let output_slices = developed.slice_times.len() - 1;
            first_slice_times.push(developed.slice_times[output_slices - 1]);
            last_slice_times.push(developed.slice_times[output_slices]);
            slowest = slowest.max(developed.slice_times.iter().max().copied().unwrap());
            slices = developed.slice_times.len();
            assert_eq!(developed.image_rgba.len(), (WIDTH * HEIGHT * 4) as usize);
            // Любая высота полосы и любой повтор дают ту же картинку 12 Мпикс бит-в-бит.
            match &first_image {
                None => first_image = Some(developed.image_rgba),
                Some(first) => assert!(
                    *first == developed.image_rgba,
                    "полоса {slice_rows}: картинка отличается"
                ),
            }
        }
        first_slice_times.sort();
        last_slice_times.sort();
        println!(
            "без validation: полоса {slice_rows}: последняя полоса вывода {:.2} мс, шаг отделки (миниатюра 256 px, переход, без копии JPEG) {:.2} мс \
             (медианы из 3, отправка + fence): отделка − полоса ≈ {:.2} мс",
            millis(first_slice_times[1]),
            millis(last_slice_times[1]),
            millis(last_slice_times[1]) - millis(first_slice_times[1]),
        );
        totals.sort();
        println!(
            "без validation: проявление, полоса {slice_rows}: {slices} полос, всего {:.1} мс (медиана из 3), самая долгая полоса {:.1} мс, плоскости {:.1} МиБ",
            millis(totals[1]),
            millis(slowest),
            developer.plane_memory_bytes() as f64 / (1 << 20) as f64,
        );
    }
}
