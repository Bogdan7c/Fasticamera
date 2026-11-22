//! Блок Exif для HEIF/JPEG: теги, значения, даты (граница суток, високосный год, до 1970).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "тесты")]

mod support;

use camera_contract::geometry::{PixelRect, PixelSize};
use photo_output::exif::{CaptureMoment, ExifInputs, RotationCarrier, build_exif_tiff};
use still_contract::shot::StillRotation;
use support::still_fixture::{camera_description, capture_moment, main_camera_stream, still_facts};
use support::tiff_reader::{Order, ParsedIfd, ParsedTiff};

fn exif_for(moment: CaptureMoment, rotation: StillRotation) -> Vec<u8> {
    let area = PixelRect {
        left: 0,
        top: 0,
        size: PixelSize::new(64, 48),
    };
    let mut facts = still_facts(main_camera_stream(area), area);
    facts.rotation = rotation;
    build_exif_tiff(&ExifInputs {
        rotation_carrier: RotationCarrier::ExifOrientationTag,
        facts: &facts,
        camera: &camera_description(),
        captured_at: &moment,
        pixel_size: PixelSize::new(4000, 3000),
    })
    .unwrap()
}

fn exif_ifd(tiff: &ParsedTiff<'_>, ifd0: &ParsedIfd) -> ParsedIfd {
    tiff.sub_ifd(ifd0.entry(34665))
}

/// (DateTimeOriginal, SubSecTimeOriginal, OffsetTimeOriginal) блока для момента.
fn time_tags(utc_millis: i64, utc_offset_minutes: i16) -> (String, String, String) {
    let bytes = exif_for(
        CaptureMoment {
            utc_millis,
            utc_offset_minutes,
        },
        StillRotation::None,
    );
    let tiff = ParsedTiff::open(&bytes);
    let ifd0 = tiff.first_ifd();
    let exif = exif_ifd(&tiff, &ifd0);
    (
        exif.entry(36867).ascii(),
        exif.entry(37521).ascii(),
        exif.entry(36881).ascii(),
    )
}

#[test]
fn block_carries_camera_exposure_lens_and_image_size_tags() {
    let bytes = exif_for(capture_moment(), StillRotation::Clockwise90);
    let tiff = ParsedTiff::open(&bytes);
    let ifd0 = tiff.first_ifd();

    assert_eq!(ifd0.tags(), vec![271, 272, 274, 282, 283, 296, 305, 306, 531, 34665]);
    assert_eq!(ifd0.entry(282).rationals(), vec![(72, 1)]);
    assert_eq!(ifd0.entry(296).shorts(), vec![2], "дюймы");
    assert_eq!(ifd0.entry(531).shorts(), vec![1], "YCbCrPositioning: центр");
    assert_eq!(ifd0.entry(271).ascii(), "OnePlus");
    assert_eq!(ifd0.entry(272).ascii(), "CPH2447");
    assert_eq!(ifd0.entry(305).ascii(), "Fasticamera 0.1");
    assert_eq!(
        ifd0.entry(274).shorts(),
        vec![6],
        "поворот 90° по часовой = Orientation 6"
    );
    assert_eq!(ifd0.entry(306).ascii(), "2026:10:25 20:48:05");

    let exif = exif_ifd(&tiff, &ifd0);
    assert_eq!(exif.entry(36864).bytes(), b"0232");
    // 22 450 523 нс = 22450523/1000000000 с.
    assert_eq!(exif.entry(33434).rationals(), vec![(22_450_523, 1_000_000_000)]);
    assert_eq!(exif.entry(34855).shorts(), vec![249]);
    assert_eq!(exif.entry(33437).rationals(), vec![(1800, 1000)]);
    assert_eq!(exif.entry(37386).rationals(), vec![(5590, 1000)]);
    assert_eq!(exif.entry(36867).ascii(), "2026:10:25 20:48:05");
    assert_eq!(exif.entry(36868).ascii(), "2026:10:25 20:48:05");
    assert_eq!(exif.entry(37521).ascii(), "250");
    assert_eq!(exif.entry(36880).ascii(), "+03:00");
    assert_eq!(exif.entry(36881).ascii(), "+03:00");
    assert_eq!(exif.entry(40961).shorts(), vec![1], "ColorSpace = sRGB");
    assert_eq!(exif.entry(40962).longs(), vec![4000]);
    assert_eq!(exif.entry(40963).longs(), vec![3000]);
    assert_eq!(exif.entry(37121).bytes(), &[1, 2, 3, 0]);
}

#[test]
fn orientation_tag_follows_every_rotation() {
    for (rotation, expected) in [
        (StillRotation::None, 1),
        (StillRotation::Clockwise90, 6),
        (StillRotation::Clockwise180, 3),
        (StillRotation::Clockwise270, 8),
    ] {
        let bytes = exif_for(capture_moment(), rotation);
        let ifd0 = ParsedTiff::open(&bytes).first_ifd();
        assert_eq!(ifd0.entry(274).shorts(), vec![expected], "{rotation:?}");
    }
}

#[test]
fn local_time_crosses_midnight_forward_with_positive_offset() {
    // 2026-12-31 22:30:00 UTC, UTC+03:00 → 2027-01-01 01:30:00.
    let (date_time, _, offset) = time_tags(1_798_756_200_000, 180);
    assert_eq!(date_time, "2027:01:01 01:30:00");
    assert_eq!(offset, "+03:00");
}

#[test]
fn local_time_crosses_midnight_backward_into_leap_day_with_negative_half_hour_offset() {
    // 2024-03-01 02:00:00 UTC, UTC-05:30 → 2024-02-29 20:30:00 (2024 високосный).
    let (date_time, _, offset) = time_tags(1_709_258_400_000, -330);
    assert_eq!(date_time, "2024:02:29 20:30:00");
    assert_eq!(offset, "-05:30");
}

#[test]
fn leap_day_and_milliseconds_are_formatted() {
    let (date_time, sub_second, _) = time_tags(1_709_208_000_123, 0);
    assert_eq!(date_time, "2024:02:29 12:00:00");
    assert_eq!(sub_second, "123");
}

#[test]
fn century_non_leap_year_has_no_february_29() {
    // 2100-03-01 00:00:00 UTC, минус одна минута смещения → 2100-02-28 23:59:00 (2100 не високосный).
    let (date_time, _, offset) = time_tags(4_107_542_400_000, -1);
    assert_eq!(date_time, "2100:02:28 23:59:00");
    assert_eq!(offset, "-00:01");
}

#[test]
fn year_2000_is_leap_because_it_divides_by_400() {
    let (date_time, sub_second, _) = time_tags(951_868_799_999, 0);
    assert_eq!(date_time, "2000:02:29 23:59:59");
    assert_eq!(sub_second, "999");
}

#[test]
fn moments_before_the_unix_epoch_are_formatted() {
    let (date_time, sub_second, _) = time_tags(-1, 0);
    assert_eq!(date_time, "1969:12:31 23:59:59");
    assert_eq!(sub_second, "999");
    let (date_time, _, _) = time_tags(0, 0);
    assert_eq!(date_time, "1970:01:01 00:00:00");
    // 1900-03-01 00:00:00 UTC (1900 не високосный): минус минута → 28 февраля.
    let (date_time, _, _) = time_tags(-2_203_891_200_000, -1);
    assert_eq!(date_time, "1900:02:28 23:59:00");
}

#[test]
fn unrepresentable_moment_drops_time_tags_but_keeps_the_rest() {
    // Больше суток и крайние значения i16 (у `i16::MIN` обычный модуль переполняется).
    for broken_offset in [30_000, i16::MAX, i16::MIN, 24 * 60 + 1, -(24 * 60 + 1)] {
        let bytes = exif_for(
            CaptureMoment {
                utc_millis: 0,
                utc_offset_minutes: broken_offset,
            },
            StillRotation::None,
        );
        let tiff = ParsedTiff::open(&bytes);
        let ifd0 = tiff.first_ifd();
        assert!(ifd0.maybe(306).is_none(), "смещение {broken_offset}");
        let exif = exif_ifd(&tiff, &ifd0);
        assert!(exif.maybe(36867).is_none(), "смещение {broken_offset}");
        assert_eq!(exif.entry(34855).shorts(), vec![249], "остальные поля на месте");
    }
    // Ровно сутки — ещё допустимо.
    assert_eq!(time_tags(0, 24 * 60).2, "+24:00");
    assert_eq!(time_tags(0, -24 * 60).2, "-24:00");
}

#[test]
fn missing_lens_and_result_values_are_omitted_not_zeroed() {
    let area = PixelRect {
        left: 0,
        top: 0,
        size: PixelSize::new(64, 48),
    };
    let mut facts = still_facts(main_camera_stream(area), area);
    facts.capture.result.exposure_time = None;
    facts.capture.result.sensitivity_iso = None;
    let mut camera = camera_description();
    camera.lens = None;
    let bytes = build_exif_tiff(&ExifInputs {
        rotation_carrier: RotationCarrier::ExifOrientationTag,
        facts: &facts,
        camera: &camera,
        captured_at: &capture_moment(),
        pixel_size: PixelSize::new(64, 48),
    })
    .unwrap();
    let tiff = ParsedTiff::open(&bytes);
    let exif = exif_ifd(&tiff, &tiff.first_ifd());
    for tag in [33434, 34855, 33437, 37386] {
        assert!(exif.maybe(tag).is_none(), "тег {tag} не должен писаться");
    }
    assert!(exif.maybe(36867).is_some());
}

#[test]
fn long_exposure_uses_millisecond_fraction_and_non_ascii_text_is_replaced() {
    let area = PixelRect {
        left: 0,
        top: 0,
        size: PixelSize::new(64, 48),
    };
    let mut facts = still_facts(main_camera_stream(area), area);
    facts.capture.result.exposure_time = Some(std::time::Duration::from_secs(10));
    let mut camera = camera_description();
    camera.model = "Модель".to_owned();
    let bytes = build_exif_tiff(&ExifInputs {
        rotation_carrier: RotationCarrier::ExifOrientationTag,
        facts: &facts,
        camera: &camera,
        captured_at: &capture_moment(),
        pixel_size: PixelSize::new(64, 48),
    })
    .unwrap();
    let tiff = ParsedTiff::open(&bytes);
    let ifd0 = tiff.first_ifd();
    assert_eq!(ifd0.entry(272).ascii(), "??????");
    let exif = exif_ifd(&tiff, &ifd0);
    assert_eq!(
        exif.entry(33434).rationals(),
        vec![(10, 1)],
        "10 с = 10/1 после сокращения"
    );
}

#[test]
fn block_is_big_endian_so_android_reads_heif_size_and_rotation_right() {
    // 14c, телефон: с блоком `II` MediaStore записал HEIF 4096×3072 как 16×12 — `ExifInterface`
    // кладёт размер из контейнера числом `MM`, а читает его в порядке блока. Сверяем сырые байты.
    let bytes = exif_for(capture_moment(), StillRotation::None);
    assert_eq!(&bytes[..4], b"MM\0*", "блок Exif — big-endian");
    let tiff = ParsedTiff::open(&bytes);
    assert_eq!(tiff.order, Order::Big);
    let exif = exif_ifd(&tiff, &tiff.first_ifd());
    let width = exif.entry(40962);
    assert_eq!(
        width.bytes(),
        &[0, 0, 0x0F, 0xA0],
        "PixelXDimension 4000 старшим байтом вперёд"
    );
    assert_eq!(width.longs(), vec![4000]);
    assert_eq!(exif.entry(40963).longs(), vec![3000]);
}

#[test]
fn container_carried_rotation_leaves_no_orientation_tag() {
    // HEIF: поворот уже лежит в `irot` контейнера. Тега нет вовсе: с `Orientation = 1` платформенный
    // `ExifInterface` перезаписывал поворот из `irot`, и MediaStore считал снимок неповёрнутым (14c).
    let area = PixelRect {
        left: 0,
        top: 0,
        size: PixelSize::new(64, 48),
    };
    for rotation in [
        StillRotation::None,
        StillRotation::Clockwise90,
        StillRotation::Clockwise180,
        StillRotation::Clockwise270,
    ] {
        let mut facts = still_facts(main_camera_stream(area), area);
        facts.rotation = rotation;
        let bytes = build_exif_tiff(&ExifInputs {
            rotation_carrier: RotationCarrier::ContainerTransform,
            facts: &facts,
            camera: &camera_description(),
            captured_at: &capture_moment(),
            pixel_size: PixelSize::new(64, 48),
        })
        .unwrap();
        let ifd0 = ParsedTiff::open(&bytes).first_ifd();
        assert!(ifd0.maybe(274).is_none(), "{rotation:?}: Orientation не пишется");
        assert_eq!(ifd0.entry(271).ascii(), "OnePlus", "остальные поля IFD0 на месте");
    }
}

/// Значение тега `Flash` (0x9209) для фактов кадра, изменённых `adjust`.
fn flash_tag_for(adjust: impl FnOnce(&mut still_contract::shot::StillFrameFacts)) -> Vec<u16> {
    let area = PixelRect {
        left: 0,
        top: 0,
        size: PixelSize::new(64, 48),
    };
    let mut facts = still_facts(main_camera_stream(area), area);
    adjust(&mut facts);
    let bytes = build_exif_tiff(&ExifInputs {
        rotation_carrier: RotationCarrier::ExifOrientationTag,
        facts: &facts,
        camera: &camera_description(),
        captured_at: &capture_moment(),
        pixel_size: PixelSize::new(4000, 3000),
    })
    .unwrap();
    let tiff = ParsedTiff::open(&bytes);
    let ifd0 = tiff.first_ifd();
    exif_ifd(&tiff, &ifd0).entry(37385).shorts()
}

#[test]
fn flash_tag_says_fired_only_for_a_flash_frame_and_no_function_for_a_camera_without_flash() {
    use camera_contract::flash::{FlashModeEcho, FlashResult, FlashShotId, FlashState};
    use still_contract::frame_choice::FrameChoice;

    // Кадр со вспышкой, прошедший проверку освещённости рендера (выбор `FlashFrame`) — «сработала».
    let fired = flash_tag_for(|facts| {
        facts.choice = FrameChoice::FlashFrame {
            sensor_timestamp: facts.sensor_timestamp,
            shot: FlashShotId(7),
        };
    });
    assert_eq!(fired, vec![0x0001]);
    // Обычный снимок камеры со вспышкой — «не сработала», даже если HAL написал FIRED на кадре:
    // `FIRED` без выбора `FlashFrame` не доказательство (факт 14d).
    let not_fired = flash_tag_for(|facts| {
        facts.capture.result.three_a.flash = FlashResult {
            mode: Some(FlashModeEcho::Off),
            state: Some(FlashState::Fired),
        };
    });
    assert_eq!(not_fired, vec![0x0000]);
    // Камера без вспышки (фронт): HAL сообщает UNAVAILABLE — «функции вспышки нет».
    let no_function = flash_tag_for(|facts| {
        facts.capture.result.three_a.flash = FlashResult {
            mode: None,
            state: Some(FlashState::Unavailable),
        };
    });
    assert_eq!(no_function, vec![0x0020]);
}
