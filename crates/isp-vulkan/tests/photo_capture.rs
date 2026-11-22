//! Стадия «захват» снимка: сырые отсчёты полезной области кадра камеры (RAW10 упакованный буфер и RAW16
//! картинка) копируются в буфер снимка БИТ-В-БИТ — u16 little-endian, строка за строкой, ровно полезная область,
//! без масштабирования; смещение полезной области и мусор в паддинге на результат не влияют.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod photo_support;
mod quality_support;

use gpu_vulkan::kit_test_support::{run_one_shot_blocking, run_one_shot_blocking_traced};
use gpu_vulkan::live_objects::LiveObjectKind;
use gpu_vulkan::raw_mosaic_preview::ValidArea;
use isp_model::cfa::BayerPattern;
use isp_vulkan::photo::{PhotoConfig, PhotoError, RawCapturer};
use photo_support::{Placement, SourceKind, capture, into_gpu_error, real_frame, upload, zone_plate_frame};
use quality_support::{assert_no_validation_errors, gpu_context_or_skip};

/// Отсчёты эталона как байты u16 little-endian по строкам — то, что обязан лежать в буфере захвата.
fn expected_bytes(samples: &[u16]) -> Vec<u8> {
    samples.iter().flat_map(|sample| sample.to_le_bytes()).collect()
}

#[test]
fn capture_copies_valid_area_bit_for_bit_from_raw16_and_raw10_with_offset_and_garbage_padding() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let frames = [
        real_frame("real-main-lamp-jar"),
        real_frame("real-main-lamp-tap-highlight"),
        zone_plate_frame(64, 40, BayerPattern::GBRG),
    ];
    // Смещения и мусор: полезная область не в углу кадра, паддинг забит белым уровнем (RAW10 ещё и байтом 0xA5).
    let placements = [
        Placement::PADDED,
        Placement {
            left: 0,
            top: 0,
            right_padding: 0,
            bottom_padding: 0,
            padding_value: 0,
        },
        Placement {
            left: 10,
            top: 8,
            right_padding: 2,
            bottom_padding: 6,
            padding_value: 0x3FF,
        },
    ];
    for frame in &frames {
        let expected = expected_bytes(frame.raw.samples());
        for placement in placements {
            for kind in [SourceKind::Raw16, SourceKind::Raw10] {
                // RAW10 требует ширину кадра кратной 4.
                if kind == SourceKind::Raw10
                    && (placement.left + frame.raw.size().width + placement.right_padding) % 4 != 0
                {
                    continue;
                }
                let gpu_frame = upload(&context, &frame.raw, kind, placement);
                let captured = capture(&context, &mut capturer, &gpu_frame);
                let samples = captured.samples().unwrap();
                assert_eq!(
                    (samples.size().width, samples.size().height),
                    (frame.raw.size().width, frame.raw.size().height)
                );
                assert_eq!(
                    samples.le_bytes().len(),
                    expected.len(),
                    "{} {kind:?} {placement:?}: размер",
                    frame.name
                );
                assert!(
                    samples.le_bytes() == expected.as_slice(),
                    "{} {kind:?} {placement:?}: байты отличаются",
                    frame.name
                );
                // Отсчёты читаются и поштучно, значения не масштабированы (10 бит).
                let (x, y) = (frame.raw.size().width - 1, frame.raw.size().height - 1);
                assert_eq!(samples.sample(x, y), Some(frame.raw.sample(x, y)));
                assert!(samples.sample(x, y).unwrap() <= 1023);
                assert_eq!(captured.valid_area(), gpu_frame.valid_area);
            }
        }
    }
    assert_no_validation_errors(&context);
}

#[test]
fn captured_samples_outlive_capturer_and_move_to_another_thread() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let frame = real_frame("real-main-lamp-towel");
    let expected = expected_bytes(frame.raw.samples());
    let samples = {
        let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
        let gpu_frame = upload(&context, &frame.raw, SourceKind::Raw16, Placement::PADDED);
        let captured = capture(&context, &mut capturer, &gpu_frame);
        let samples = captured.samples().unwrap();
        drop(captured);
        samples
    };
    // Захватчик, кадр и RawCapture уничтожены; байты живы, пока жив RawSamples16, и читаются с другого потока,
    // где же буфер и освобождается (последний владелец).
    let bytes_equal = std::thread::spawn(move || samples.le_bytes() == expected.as_slice())
        .join()
        .unwrap();
    assert!(bytes_equal);
    assert_eq!(
        context.live_objects(),
        baseline,
        "буфер захвата освобождён вместе с последним владельцем"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn second_capture_before_completion_is_busy_and_does_not_disturb_the_first() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let frame = real_frame("real-main-lamp-jar");
    let gpu_frame = upload(&context, &frame.raw, SourceKind::Raw16, Placement::PADDED);
    let mut pending = None;
    let first_run = run_one_shot_blocking_traced(&context, |command_buffer| {
        pending = Some(
            capturer
                .record_capture(command_buffer, &gpu_frame.source, gpu_frame.valid_area)
                .map_err(into_gpu_error)?,
        );
        Ok(())
    })
    .unwrap();
    // Второй захват, пока первый не подтверждён.
    let second = capturer.record_capture(ash::vk::CommandBuffer::null(), &gpu_frame.source, gpu_frame.valid_area);
    assert!(matches!(second, Err(PhotoError::Busy { .. })), "{second:?}");
    // Первый подтверждается и содержит верные байты.
    let mut pending = pending.unwrap();
    pending.note_submitted(&first_run.work);
    let captured = capturer.complete_after_fence(pending, &first_run.proof).unwrap();
    assert!(captured.samples().unwrap().le_bytes() == expected_bytes(frame.raw.samples()).as_slice());
    // Подтверждение чужого/повторного захвата — нарушение порядка, а не молчаливый успех.
    let mut other = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let mut stray = None;
    let stray_proof = run_one_shot_blocking(&context, |command_buffer| {
        stray = Some(
            other
                .record_capture(command_buffer, &gpu_frame.source, gpu_frame.valid_area)
                .map_err(into_gpu_error)?,
        );
        Ok(())
    })
    .unwrap();
    let wrong_owner = capturer.complete_after_fence(stray.unwrap(), &stray_proof);
    assert!(
        matches!(wrong_owner, Err(PhotoError::OutOfOrder { .. })),
        "{wrong_owner:?}"
    );
    assert_no_validation_errors(&context);
}

/// Подтверждение захвата связано с работой, что его записала: доказательство без отметки записавшей работы или
/// чужой работы — отказ `SealRefused` (буфер, в который GPU мог ещё писать, не запечатывается и не уничтожается).
#[test]
fn capture_completion_refuses_proof_that_is_not_of_the_recording_work() {
    let Some(context) = gpu_context_or_skip() else { return };
    let frame = real_frame("real-main-lamp-jar");
    let gpu_frame = upload(&context, &frame.raw, SourceKind::Raw16, Placement::PADDED);
    for case in ["без отметки работы", "доказательство чужой работы"] {
        let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
        let mut pending = None;
        let run = run_one_shot_blocking_traced(&context, |command_buffer| {
            pending = Some(
                capturer
                    .record_capture(command_buffer, &gpu_frame.source, gpu_frame.valid_area)
                    .map_err(into_gpu_error)?,
            );
            Ok(())
        })
        .unwrap();
        let mut pending = pending.unwrap();
        let proof = if case == "без отметки работы" {
            run.proof.clone()
        } else {
            pending.note_submitted(&run.work);
            // Доказательство другой, настоящей завершённой работы.
            run_one_shot_blocking(&context, |_| Ok(())).unwrap()
        };
        let refused = capturer.complete_after_fence(pending, &proof);
        assert!(
            matches!(refused, Err(PhotoError::SealRefused { .. })),
            "{case}: {refused:?}"
        );
    }
}

#[test]
fn invalid_valid_areas_are_refused_before_any_command_and_leave_capturer_usable() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline_buffers = context.live_objects().count(LiveObjectKind::Buffer);
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let frame = real_frame("real-main-lamp-jar");
    let gpu_frame = upload(&context, &frame.raw, SourceKind::Raw16, Placement::PADDED);
    let area = gpu_frame.valid_area;
    let buffers_with_frame = context.live_objects().count(LiveObjectKind::Buffer);
    assert!(buffers_with_frame >= baseline_buffers);
    let broken = [
        ValidArea {
            left: area.left + 1,
            ..area
        },
        ValidArea {
            width: area.width + 1,
            ..area
        },
        ValidArea { height: 0, ..area },
        ValidArea {
            width: area.width + 8,
            ..area
        },
        ValidArea {
            top: area.top + 100,
            ..area
        },
    ];
    for bad in broken {
        // Нулевой командный буфер: ошибка обязана прийти ДО записи команд, иначе драйвер упал бы.
        let result = capturer.record_capture(ash::vk::CommandBuffer::null(), &gpu_frame.source, bad);
        assert!(
            matches!(result, Err(PhotoError::InvalidInput { .. })),
            "{bad:?}: {result:?}"
        );
        assert_eq!(
            context.live_objects().count(LiveObjectKind::Buffer),
            buffers_with_frame,
            "{bad:?}: отказ не должен оставлять буферов"
        );
    }
    // После отказов захватчик работает.
    let captured = capture(&context, &mut capturer, &gpu_frame);
    assert!(captured.samples().unwrap().le_bytes() == expected_bytes(frame.raw.samples()).as_slice());
    assert_no_validation_errors(&context);
}

#[test]
fn unsubmitted_capture_can_be_discarded_and_frees_the_buffer() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let frame = real_frame("real-main-lamp-jar");
    let gpu_frame = upload(&context, &frame.raw, SourceKind::Raw16, Placement::PADDED);
    let before = context.live_objects();
    let mut pending = None;
    // Записываем, но командный буфер не отправляем (разовая отправка пустая).
    run_one_shot_blocking(&context, |command_buffer| {
        pending = Some(
            capturer
                .record_capture(command_buffer, &gpu_frame.source, gpu_frame.valid_area)
                .map_err(into_gpu_error)?,
        );
        Ok(())
    })
    .unwrap();
    assert!(context.live_objects().count(LiveObjectKind::Buffer) > before.count(LiveObjectKind::Buffer));
    capturer.discard_unsubmitted(pending.unwrap()).unwrap();
    assert_eq!(context.live_objects(), before, "буфер освобождён, захватчик свободен");
    // Захватчик снова доступен.
    let captured = capture(&context, &mut capturer, &gpu_frame);
    assert_eq!(captured.size().width, frame.raw.size().width);
    assert_no_validation_errors(&context);
}

#[test]
fn prepared_buffer_is_used_by_the_next_capture_and_keeps_record_free_of_allocation() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut capturer = RawCapturer::new(&context, &PhotoConfig::default()).unwrap();
    let frame = real_frame("real-main-lamp-towel");
    let gpu_frame = upload(&context, &frame.raw, SourceKind::Raw10, Placement::PADDED);
    let size = frame.raw.size();
    assert!(!capturer.has_prepared_buffer());

    let before_prepare = context.live_objects().count(LiveObjectKind::Buffer);
    capturer.prepare(size).unwrap();
    assert!(capturer.has_prepared_buffer());
    assert_eq!(
        context.live_objects().count(LiveObjectKind::Buffer),
        before_prepare + 1,
        "буфер выделен заранее"
    );
    // Повторный prepare того же размера ничего не выделяет.
    capturer.prepare(size).unwrap();
    assert_eq!(context.live_objects().count(LiveObjectKind::Buffer), before_prepare + 1);

    // Запись берёт подготовленный буфер: новых буферов нет, а результат верен.
    let mut pending = None;
    let run = run_one_shot_blocking_traced(&context, |command_buffer| {
        pending = Some(
            capturer
                .record_capture(command_buffer, &gpu_frame.source, gpu_frame.valid_area)
                .map_err(into_gpu_error)?,
        );
        Ok(())
    })
    .unwrap();
    assert!(!capturer.has_prepared_buffer(), "буфер перешёл к захвату");
    assert_eq!(
        context.live_objects().count(LiveObjectKind::Buffer),
        before_prepare + 1,
        "record не выделяет"
    );
    let mut pending = pending.unwrap();
    pending.note_submitted(&run.work);
    let captured = capturer.complete_after_fence(pending, &run.proof).unwrap();
    assert!(captured.samples().unwrap().le_bytes() == expected_bytes(frame.raw.samples()).as_slice());

    // Подготовленный буфер ДРУГОГО размера не подсовывается: запись выделяет свой, подходящий.
    capturer.prepare(isp_vulkan_pixel_size(8, 8)).unwrap();
    let captured = capture(&context, &mut capturer, &gpu_frame);
    assert!(captured.samples().unwrap().le_bytes() == expected_bytes(frame.raw.samples()).as_slice());
    assert_no_validation_errors(&context);
}

fn isp_vulkan_pixel_size(width: u32, height: u32) -> camera_contract::geometry::PixelSize {
    camera_contract::geometry::PixelSize::new(width, height)
}
