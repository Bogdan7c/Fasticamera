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
    DevelopRequest, DevelopTargets, PhotoConfig, PhotoDeveloper, PhotoError, RawCapturer, RgbaReadback, SliceOutcome,
};
use photo_support::{
    Placement, SourceKind, capture, compare_rgba, crop_in_buffer, develop, develop_with_targets, developer_for,
    into_gpu_error, upload, zone_plate_frame,
};
use quality_support::{assert_no_validation_errors, gpu_context_or_skip};
use still_contract::shot::StillRotation;

/// План миниатюры при конфиге по умолчанию и без поворота (цели проявки заказываются под него).
fn plan_for(size: PixelSize) -> isp_vulkan::photo::ThumbnailPlan {
    PhotoConfig::default()
        .thumbnail_plan(size, StillRotation::None)
        .unwrap()
}

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
        rotation: StillRotation::None,
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
    // Шагов записи: 2 полосы вывода + шаг отделки (миниатюра и копии, 14g).
    assert_eq!(start.slice_count, 3);
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
    assert_eq!(outcome, Some(SliceOutcome::Recorded { remaining: 2 }));
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

    // Вторая (последняя) полоса вывода: осталась отделка, проявку закончить нельзя.
    let second_run = run_one_shot_blocking_traced(&context, |command_buffer| {
        outcome = Some(developer.record_next_slice(command_buffer).map_err(into_gpu_error)?);
        Ok(())
    })
    .unwrap();
    assert_eq!(outcome, Some(SliceOutcome::Recorded { remaining: 1 }));
    developer.note_slice_submitted(&second_run.work).unwrap();
    developer.slice_completed(&second_run.proof).unwrap();
    assert!(
        matches!(developer.finish(), Err(PhotoError::OutOfOrder { .. })),
        "полосы вывода подтверждены, но шаг отделки не записан"
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
            rotation: StillRotation::None,
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
            rotation: StillRotation::None,
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
            rotation: StillRotation::None,
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
        rotation: StillRotation::None,
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
                rotation: StillRotation::None,
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

#[test]
fn targets_allocated_on_another_thread_develop_the_same_picture_as_begin() {
    // Решение 144: картинку и копию для JPEG выделяет мастерская снимка (другой поток), рендер только привязывает.
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let frame = zone_plate_frame(WIDTH, HEIGHT, BayerPattern::BGGR);
    let gpu_frame = upload(&context, &frame.raw, SourceKind::Raw10, Placement::PADDED);
    let captured = capture(&context, &mut capturer, &gpu_frame);
    let mut developer = developer_for(&context, 16, WIDTH);
    let crop = crop_in_buffer(gpu_frame.valid_area, 7, 5, 33, 29);
    let reference = develop(&context, &mut developer, &captured, &frame, crop, RgbaReadback::ForJpeg);

    let worker_context = context.clone();
    let targets = std::thread::spawn(move || {
        DevelopTargets::allocate(
            &worker_context,
            PixelSize::new(33, 29),
            RgbaReadback::ForJpeg,
            plan_for(PixelSize::new(33, 29)),
        )
    })
    .join()
    .unwrap()
    .unwrap();
    let request = DevelopRequest {
        raw: &captured,
        params: &frame.params,
        crop,
        readback: RgbaReadback::ForJpeg,
        rotation: StillRotation::None,
    };
    let prepared = develop_with_targets(&context, &mut developer, &request, targets);
    assert_eq!(prepared.image_rgba, reference.image_rgba, "та же картинка бит-в-бит");
    assert_eq!(
        prepared.photo.host_rgba8().map(|copy| copy.rgba_bytes().to_vec()),
        reference.photo.host_rgba8().map(|copy| copy.rgba_bytes().to_vec()),
        "та же копия для JPEG"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn targets_that_do_not_fit_the_request_are_refused_without_starting_a_development() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let frame = zone_plate_frame(WIDTH, HEIGHT, BayerPattern::BGGR);
    let gpu_frame = upload(&context, &frame.raw, SourceKind::Raw10, Placement::PADDED);
    let captured = capture(&context, &mut capturer, &gpu_frame);
    let mut developer = developer_for(&context, 16, WIDTH);
    let crop = crop_in_buffer(gpu_frame.valid_area, 0, 0, 32, 24);
    let request = |readback| DevelopRequest {
        raw: &captured,
        params: &frame.params,
        crop,
        readback,
        rotation: StillRotation::None,
    };
    // Не тот размер картинки.
    let wrong_size = DevelopTargets::allocate(
        &context,
        PixelSize::new(30, 24),
        RgbaReadback::NotNeeded,
        plan_for(PixelSize::new(30, 24)),
    )
    .unwrap();
    let error = developer
        .begin_with_targets(&request(RgbaReadback::NotNeeded), wrong_size)
        .unwrap_err();
    assert!(matches!(error, PhotoError::InvalidInput { .. }), "{error:?}");
    assert!(!developer.is_developing());
    // Копия для JPEG нужна, а в целях её нет — и наоборот.
    let no_copy = DevelopTargets::allocate(&context, crop.size, RgbaReadback::NotNeeded, plan_for(crop.size)).unwrap();
    assert!(matches!(
        developer.begin_with_targets(&request(RgbaReadback::ForJpeg), no_copy),
        Err(PhotoError::InvalidInput { .. })
    ));
    // План миниатюры другой (цели под поворот на 90°, а проявка без поворота): размеры миниатюры не сходятся.
    let turned_plan = PhotoConfig::default()
        .thumbnail_plan(crop.size, StillRotation::Clockwise90)
        .unwrap();
    let wrong_thumbnail = DevelopTargets::allocate(&context, crop.size, RgbaReadback::NotNeeded, turned_plan).unwrap();
    assert!(matches!(
        developer.begin_with_targets(&request(RgbaReadback::NotNeeded), wrong_thumbnail),
        Err(PhotoError::InvalidInput { .. })
    ));
    assert!(!developer.is_developing());
    let extra_copy = DevelopTargets::allocate(&context, crop.size, RgbaReadback::ForJpeg, plan_for(crop.size)).unwrap();
    assert!(matches!(
        developer.begin_with_targets(&request(RgbaReadback::NotNeeded), extra_copy),
        Err(PhotoError::InvalidInput { .. })
    ));
    assert!(!developer.is_developing());
    // Проверка запроса без выделений: негодная область отвергается ДО заказа целей.
    let outside = PixelRect {
        left: gpu_frame.valid_area.left + 60,
        top: gpu_frame.valid_area.top,
        size: PixelSize::new(8, 8),
    };
    let checked = developer.check_request(&DevelopRequest {
        crop: outside,
        ..request(RgbaReadback::NotNeeded)
    });
    assert!(matches!(checked, Err(PhotoError::InvalidInput { .. })), "{checked:?}");
    assert!(developer.check_request(&request(RgbaReadback::NotNeeded)).is_ok());
    assert_no_validation_errors(&context);
}

/// Состояние после записи всех полос вывода и подтверждения их fence-ов, но до шага отделки (общий кусок двух
/// тестов отделки): миниатюра и копия ещё не записаны.
struct AfterOutputSlices {
    context: gpu_vulkan::context::GpuContext,
    developer: PhotoDeveloper,
    output_run: gpu_vulkan::kit_test_support::BlockingRun,
    output_steps: usize,
}

fn develop_output_slices_in_one_buffer() -> Option<AfterOutputSlices> {
    let context = gpu_context_or_skip()?;
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let frame = zone_plate_frame(WIDTH, HEIGHT, BayerPattern::BGGR);
    let gpu_frame = upload(&context, &frame.raw, SourceKind::Raw10, Placement::PADDED);
    let captured = capture(&context, &mut capturer, &gpu_frame);
    let mut developer = developer_for(&context, 16, WIDTH);
    let crop = crop_in_buffer(gpu_frame.valid_area, 0, 0, WIDTH, HEIGHT);
    let start = developer
        .begin(&DevelopRequest {
            raw: &captured,
            params: &frame.params,
            crop,
            readback: RgbaReadback::ForJpeg,
            rotation: StillRotation::None,
        })
        .unwrap();
    // Полос вывода столько, сколько строк кадра в полосах по 16; ещё один шаг записи — отделка.
    let output_steps = start.slice_count - 1;
    assert_eq!(
        output_steps,
        (HEIGHT as usize).div_ceil(16),
        "шаги = полосы вывода + отделка"
    );
    let output_run = run_one_shot_blocking_traced(&context, |command_buffer| {
        for step in 0..output_steps {
            let outcome = developer.record_next_slice(command_buffer).map_err(into_gpu_error)?;
            assert_eq!(
                outcome,
                SliceOutcome::Recorded {
                    remaining: start.slice_count - step - 1
                }
            );
        }
        Ok(())
    })
    .unwrap();
    // Все полосы вывода записаны в буфер, но проявка не закончена: остался шаг отделки.
    developer.note_slice_submitted(&output_run.work).unwrap();
    for _ in 0..output_steps {
        developer.slice_completed(&output_run.proof).unwrap();
    }
    assert!(
        matches!(developer.finish(), Err(PhotoError::OutOfOrder { .. })),
        "без шага отделки проявка не заканчивается: копии не записаны"
    );
    Some(AfterOutputSlices {
        context,
        developer,
        output_run,
        output_steps,
    })
}

/// Миниатюра и копия для JPEG записываются ОТДЕЛЬНЫМ шагом, не в последней полосе вывода: после полос вывода
/// шаг отделки в своём командном буфере доводит проявку до конца, и миниатюра равна эталону картинки.
#[test]
fn thumbnail_and_host_copy_come_from_their_own_finishing_step() {
    let Some(mut state) = develop_output_slices_in_one_buffer() else {
        return;
    };
    let finishing = run_one_shot_blocking_traced(&state.context, |command_buffer| {
        let outcome = state
            .developer
            .record_next_slice(command_buffer)
            .map_err(into_gpu_error)?;
        assert_eq!(outcome, SliceOutcome::Recorded { remaining: 0 });
        Ok(())
    })
    .unwrap();
    state.developer.note_slice_submitted(&finishing.work).unwrap();
    state.developer.slice_completed(&finishing.proof).unwrap();
    let mut photo = state.developer.finish().unwrap();
    let image = photo.gpu_image().read_back_blocking().unwrap();
    let host = photo.host_rgba8().expect("копия для JPEG запрошена");
    assert_eq!(host.rgba_bytes(), image.as_slice(), "копия записана шагом отделки");
    let isp_vulkan::photo::ThumbnailOutcome::Ready { size, rgba } = photo.take_thumbnail() else {
        panic!("миниатюра обязана быть");
    };
    let (want_size, want) =
        isp_vulkan::photo::reference_thumbnail(&image, PixelSize::new(WIDTH, HEIGHT), StillRotation::None, 256)
            .unwrap();
    assert_eq!(size, want_size);
    assert!(rgba.iter().zip(&want).all(|(got, want)| got.abs_diff(*want) <= 1));
    assert_eq!(state.output_steps, 3);
    assert_no_validation_errors(&state.context);
}

/// Буферы хоста запечатываются доказательством ШАГА ОТДЕЛКИ: доказательство командного буфера полос вывода их не
/// запечатывает. Если бы копии лежали в последней полосе вывода, оно подошло бы, и тест не упал бы.
#[test]
fn host_copy_is_not_sealed_by_the_proof_of_the_output_slices() {
    let Some(mut state) = develop_output_slices_in_one_buffer() else {
        return;
    };
    let finishing = run_one_shot_blocking_traced(&state.context, |command_buffer| {
        state
            .developer
            .record_next_slice(command_buffer)
            .map_err(into_gpu_error)?;
        Ok(())
    })
    .unwrap();
    // Отметка работы отделки подменена работой полос вывода (как если бы копии записала последняя полоса).
    state.developer.note_slice_submitted(&state.output_run.work).unwrap();
    state.developer.slice_completed(&finishing.proof).unwrap();
    assert!(
        matches!(state.developer.finish(), Err(PhotoError::SealRefused { .. })),
        "доказательство отделки не подходит к буферу, отмеченному работой полос вывода"
    );
}
