//! Протокол проявителя и выход: порядок вызовов (busy / вне очереди / ошибки входа не оставляют следов),
//! копия RGBA8 в память хоста (JPEG) совпадает с картинкой GPU, учёт объектов Vulkan, утечка при уничтожении
//! с неподтверждённой полосой.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod photo_support;
mod quality_support;

use ash::vk;
use camera_contract::geometry::{PixelRect, PixelSize};
use gpu_vulkan::kit_test_support::{run_one_shot_blocking, run_one_shot_blocking_traced};
use isp_model::cfa::BayerPattern;
use isp_model::demosaic::DemosaicMethod;
use isp_model::params::IspParamsVersion;
use isp_vulkan::photo::{
    DevelopRequest, PhotoConfig, PhotoDeveloper, PhotoError, RawCapturer, RgbaReadback, SliceOutcome,
};
use photo_support::{
    Placement, SourceKind, capture, compare_rgba, crop_in_buffer, develop, developer_for, into_gpu_error, upload,
    zone_plate_frame,
};
use quality_support::{assert_no_validation_errors, gpu_context_or_skip};

const WIDTH: u32 = 64;
const HEIGHT: u32 = 48;

#[test]
fn host_copy_for_jpeg_equals_the_gpu_image_and_is_absent_when_not_requested() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let frame = zone_plate_frame(WIDTH, HEIGHT, BayerPattern::BGGR);
    let gpu_frame = upload(&context, &frame.raw, SourceKind::Raw10, Placement::PADDED);
    let captured = capture(&context, &mut capturer, &gpu_frame);
    let mut developer = developer_for(&context, 16, WIDTH);
    for (left, top, width, height) in [(0, 0, WIDTH, HEIGHT), (7, 5, 33, 29)] {
        let crop = crop_in_buffer(gpu_frame.valid_area, left, top, width, height);
        let mut with_copy = develop(&context, &mut developer, &captured, &frame, crop, RgbaReadback::ForJpeg);
        let host = with_copy.photo.host_rgba8().expect("копия запрошена");
        assert_eq!(host.size(), PixelSize::new(width, height));
        // Независимое чтение картинки GPU (другой буфер, другой сабмит) совпадает с копией, записанной в той же
        // последней полосе.
        let difference = compare_rgba(host.rgba_bytes(), &with_copy.image_rgba, width);
        assert_eq!(difference.differing_bytes, 0, "{difference:?}");
        assert!(host.rgba_bytes().chunks_exact(4).all(|pixel| pixel[3] == 255));
        assert!(
            host.rgba_bytes().iter().any(|byte| *byte != 0) && host.rgba_bytes().iter().any(|byte| *byte != 255),
            "копия не вырождена"
        );
        // Копия переживает проявитель и читается с другого потока.
        let taken = with_copy.photo.take_host_rgba8().unwrap();
        assert!(with_copy.photo.host_rgba8().is_none());
        let expected = with_copy.image_rgba.clone();
        let equal = std::thread::spawn(move || taken.rgba_bytes() == expected.as_slice())
            .join()
            .unwrap();
        assert!(equal);

        let without = develop(
            &context,
            &mut developer,
            &captured,
            &frame,
            crop,
            RgbaReadback::NotNeeded,
        );
        assert!(
            without.photo.host_rgba8().is_none(),
            "без запроса память хоста не тратится"
        );
        assert_eq!(
            without.image_rgba, with_copy.image_rgba,
            "запрос копии не меняет картинку"
        );
    }
    assert_no_validation_errors(&context);
}

#[test]
fn calls_out_of_order_are_explicit_errors_and_do_not_corrupt_the_development() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let frame = zone_plate_frame(WIDTH, HEIGHT, BayerPattern::RGGB);
    let gpu_frame = upload(&context, &frame.raw, SourceKind::Raw16, Placement::PADDED);
    let captured = capture(&context, &mut capturer, &gpu_frame);
    let mut developer = developer_for(&context, 24, WIDTH);
    let crop = crop_in_buffer(gpu_frame.valid_area, 0, 0, WIDTH, HEIGHT);
    let request = DevelopRequest {
        raw: &captured,
        params: &frame.params,
        crop,
        readback: RgbaReadback::NotNeeded,
    };
    // Настоящее доказательство завершения (пустая работа): подтверждать им при отсутствии записанных полос нельзя.
    let spare_proof = run_one_shot_blocking(&context, |_| Ok(())).unwrap();
    // Нет активной проявки.
    assert!(matches!(
        developer.record_next_slice(vk::CommandBuffer::null()),
        Err(PhotoError::OutOfOrder { .. })
    ));
    assert!(matches!(
        developer.slice_completed(&spare_proof),
        Err(PhotoError::OutOfOrder { .. })
    ));
    assert!(matches!(developer.finish(), Err(PhotoError::OutOfOrder { .. })));
    developer.cancel().unwrap();

    let start = developer.begin(&request).unwrap();
    assert_eq!(start.slice_count, 2);
    assert!(
        matches!(developer.begin(&request), Err(PhotoError::Busy { .. })),
        "вторая проявка сразу"
    );
    assert!(
        matches!(developer.finish(), Err(PhotoError::OutOfOrder { .. })),
        "полосы не записаны"
    );
    assert!(
        matches!(
            developer.slice_completed(&spare_proof),
            Err(PhotoError::OutOfOrder { .. })
        ),
        "нечего подтверждать"
    );

    // Первая полоса записана, но не подтверждена: отмена и finish невозможны.
    let mut outcome = None;
    let first_run = run_one_shot_blocking_traced(&context, |command_buffer| {
        outcome = Some(developer.record_next_slice(command_buffer).map_err(into_gpu_error)?);
        Ok(())
    })
    .unwrap();
    assert_eq!(outcome, Some(SliceOutcome::Recorded { remaining: 1 }));
    assert!(matches!(developer.cancel(), Err(PhotoError::Busy { .. })));
    assert!(matches!(developer.finish(), Err(PhotoError::OutOfOrder { .. })));
    developer.note_slice_submitted(&first_run.work).unwrap();
    developer.slice_completed(&first_run.proof).unwrap();
    assert!(
        matches!(
            developer.slice_completed(&first_run.proof),
            Err(PhotoError::OutOfOrder { .. })
        ),
        "подтверждено дважды"
    );

    let last_run = run_one_shot_blocking_traced(&context, |command_buffer| {
        outcome = Some(developer.record_next_slice(command_buffer).map_err(into_gpu_error)?);
        Ok(())
    })
    .unwrap();
    assert_eq!(outcome, Some(SliceOutcome::Recorded { remaining: 0 }));
    // Все полосы записаны: ещё одна — `Finished` и командный буфер не тронут (нулевой хэндл не упал бы иначе).
    assert_eq!(
        developer.record_next_slice(vk::CommandBuffer::null()).unwrap(),
        SliceOutcome::Finished
    );
    assert!(
        matches!(developer.finish(), Err(PhotoError::OutOfOrder { .. })),
        "последняя без fence"
    );
    developer.note_slice_submitted(&last_run.work).unwrap();
    developer.slice_completed(&last_run.proof).unwrap();
    let photo = developer.finish().unwrap();
    assert_eq!(photo.size(), PixelSize::new(WIDTH, HEIGHT));
    assert!(!developer.is_developing());
    assert!(
        matches!(developer.finish(), Err(PhotoError::OutOfOrder { .. })),
        "finish дважды"
    );
    assert_no_validation_errors(&context);
}

/// Копия в память хоста запечатывается только доказательством работы, что записала ПОСЛЕДНЮЮ полосу: без
/// `note_slice_submitted` (проявитель не знает записавшую работу) `finish` отказывает, а не верит на слово.
#[test]
fn finish_with_host_copy_refuses_when_the_writing_work_was_not_reported() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let frame = zone_plate_frame(WIDTH, HEIGHT, BayerPattern::RGGB);
    let gpu_frame = upload(&context, &frame.raw, SourceKind::Raw16, Placement::PADDED);
    let captured = capture(&context, &mut capturer, &gpu_frame);
    let mut developer = developer_for(&context, 24, WIDTH);
    let crop = crop_in_buffer(gpu_frame.valid_area, 0, 0, WIDTH, HEIGHT);
    let start = developer
        .begin(&DevelopRequest {
            raw: &captured,
            params: &frame.params,
            crop,
            readback: RgbaReadback::ForJpeg,
        })
        .unwrap();
    for _ in 0..start.slice_count {
        let run = run_one_shot_blocking_traced(&context, |command_buffer| {
            developer.record_next_slice(command_buffer).map_err(into_gpu_error)?;
            Ok(())
        })
        .unwrap();
        // Отметка работы пропущена нарочно.
        developer.slice_completed(&run.proof).unwrap();
    }
    let refused = developer.finish();
    assert!(matches!(refused, Err(PhotoError::SealRefused { .. })), "{refused:?}");
}

#[test]
fn bad_requests_are_refused_without_leaving_a_development_or_objects() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let frame = zone_plate_frame(WIDTH, HEIGHT, BayerPattern::GBRG);
    let gpu_frame = upload(&context, &frame.raw, SourceKind::Raw16, Placement::PADDED);
    let captured = capture(&context, &mut capturer, &gpu_frame);
    let mut developer = developer_for(&context, 16, WIDTH);
    let baseline = context.live_objects();
    let area = gpu_frame.valid_area;

    let outside_crops = [
        // левее полезной области (в паддинге буфера кадра)
        PixelRect {
            left: area.left - 2,
            top: area.top,
            size: PixelSize::new(8, 8),
        },
        // выше полезной области
        PixelRect {
            left: area.left,
            top: area.top - 2,
            size: PixelSize::new(8, 8),
        },
        // выходит за правый край
        PixelRect {
            left: area.left + 60,
            top: area.top,
            size: PixelSize::new(8, 8),
        },
        // пустой
        PixelRect {
            left: area.left,
            top: area.top,
            size: PixelSize::new(0, 8),
        },
    ];
    for crop in outside_crops {
        let result = developer.begin(&DevelopRequest {
            raw: &captured,
            params: &frame.params,
            crop,
            readback: RgbaReadback::ForJpeg,
        });
        assert!(
            matches!(result, Err(PhotoError::InvalidInput { .. })),
            "{crop:?}: {result:?}"
        );
        assert!(!developer.is_developing());
    }

    // Параметры, которых GPU не умеет: демозаик не Quality и чужая версия.
    let crop = crop_in_buffer(area, 0, 0, 16, 16);
    let mut preview = frame.params.clone();
    preview.demosaic = DemosaicMethod::Preview;
    let mut old = frame.params.clone();
    old.version = IspParamsVersion(0);
    for params in [&preview, &old] {
        let result = developer.begin(&DevelopRequest {
            raw: &captured,
            params,
            crop,
            readback: RgbaReadback::NotNeeded,
        });
        assert!(
            matches!(result, Err(PhotoError::UnsupportedParams { .. })),
            "{result:?}"
        );
        assert!(!developer.is_developing());
    }
    assert_eq!(context.live_objects(), baseline, "отказы не создают объектов Vulkan");

    // Проявитель, рассчитанный на более узкий кадр, отказывает честно.
    let mut narrow = developer_for(&context, 16, 32);
    let result = narrow.begin(&DevelopRequest {
        raw: &captured,
        params: &frame.params,
        crop: crop_in_buffer(area, 0, 0, WIDTH, HEIGHT),
        readback: RgbaReadback::NotNeeded,
    });
    assert!(matches!(result, Err(PhotoError::InvalidInput { .. })), "{result:?}");

    // После всех отказов проявитель работает.
    let developed = develop(
        &context,
        &mut developer,
        &captured,
        &frame,
        crop,
        RgbaReadback::NotNeeded,
    );
    assert_eq!(developed.photo.size(), PixelSize::new(16, 16));
    assert_no_validation_errors(&context);
}

#[test]
fn finished_development_returns_all_objects_but_the_photo_and_a_dropped_busy_developer_leaks_on_purpose() {
    let Some(context) = gpu_context_or_skip() else { return };
    let frame = zone_plate_frame(WIDTH, HEIGHT, BayerPattern::RGGB);
    let gpu_frame = upload(&context, &frame.raw, SourceKind::Raw16, Placement::PADDED);
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let captured = capture(&context, &mut capturer, &gpu_frame);
    let crop = crop_in_buffer(gpu_frame.valid_area, 0, 0, WIDTH, HEIGHT);

    // Обычный путь: после уничтожения проявителя и снимка счётчики возвращаются к базовым.
    let baseline = context.live_objects();
    {
        let mut developer = developer_for(&context, 16, WIDTH);
        let developed = develop(&context, &mut developer, &captured, &frame, crop, RgbaReadback::ForJpeg);
        drop(developed);
    }
    assert_eq!(context.live_objects(), baseline, "всё освобождено");

    // Проявитель с неподтверждённой полосой при уничтожении теряется намеренно (GPU мог ещё писать).
    {
        let mut developer: PhotoDeveloper = developer_for(&context, 16, WIDTH);
        developer
            .begin(&DevelopRequest {
                raw: &captured,
                params: &frame.params,
                crop,
                readback: RgbaReadback::NotNeeded,
            })
            .unwrap();
        run_one_shot_blocking(&context, |command_buffer| {
            developer.record_next_slice(command_buffer).map_err(into_gpu_error)?;
            Ok(())
        })
        .unwrap();
    }
    assert_ne!(context.live_objects(), baseline, "утечка видна в учёте живых объектов");
    assert_no_validation_errors(&context);
}
