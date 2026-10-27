use camera_contract::geometry::PixelSize;
use camera_contract::viewfinder_crop::{CropAspect, ZoomFactor};
use gpu_vulkan::hardware_buffer_cache::queue_handoff::{BufferAcquire, ImageAcquire, ReleaseTarget};

use super::*;

fn rect(left: u32, top: u32, width: u32, height: u32) -> PixelRect {
    PixelRect {
        left,
        top,
        size: PixelSize::new(width, height),
    }
}

fn raw16(width: u32, height: u32) -> MosaicSource {
    MosaicSource::SampledR16 {
        view: vk::ImageView::null(),
        image: vk::Image::null(),
        width,
        height,
        acquire: ImageAcquire::FromForeignQueue,
        release: ReleaseTarget::KeepOwned,
    }
}

fn raw10(width: u32, height: u32, row_stride_bytes: u32, size_bytes: u64) -> MosaicSource {
    MosaicSource::PackedRaw10 {
        buffer: vk::Buffer::null(),
        size_bytes,
        row_stride_bytes,
        width,
        height,
        acquire: BufferAcquire::AlreadyOwned,
        release: ReleaseTarget::KeepOwned,
    }
}

fn geometry(valid_area: PixelRect, crop: PixelRect, orientation: SensorOrientation) -> ViewfinderGeometry {
    ViewfinderGeometry {
        valid_area,
        crop,
        sensor_orientation: orientation,
    }
}

#[test]
fn output_is_half_the_crop_and_swaps_for_quarter_turns() {
    let area = rect(0, 0, 4096, 3072);
    for (orientation, expected) in [
        (SensorOrientation::Degrees0, (2048, 1536)),
        (SensorOrientation::Degrees90, (1536, 2048)),
        (SensorOrientation::Degrees180, (2048, 1536)),
        (SensorOrientation::Degrees270, (1536, 2048)),
    ] {
        let checked = check_geometry(&geometry(area, area, orientation), &raw16(4096, 3072)).unwrap();
        assert_eq!(
            (checked.output.width, checked.output.height),
            expected,
            "{orientation:?}"
        );
        assert_eq!(checked.quarter_turns, orientation.quarter_turns_clockwise());
        assert_eq!(checked.raw10_row_stride_bytes, 0, "RAW16 шаг строки не использует");
    }
}

#[test]
fn from_crop_resolves_aspect_and_zoom() {
    let area = rect(0, 0, 4096, 3072);
    let crop = ViewfinderCrop {
        aspect: CropAspect::SIXTEEN_BY_NINE,
        zoom: ZoomFactor::new(2.0).unwrap(),
    };
    let resolved = ViewfinderGeometry::from_crop(area, &crop, SensorOrientation::Degrees90).unwrap();
    assert_eq!(resolved.crop, rect(1024, 960, 2048, 1152));
    assert_eq!(
        resolved.output_extent(),
        vk::Extent2D {
            width: 576,
            height: 1024
        }
    );
}

#[test]
fn crop_outside_valid_area_or_misaligned_is_rejected() {
    // 16:9 основной: строки 2304+ — мусор; кадрирование туда залезть не может.
    let area = rect(0, 0, 4096, 2304);
    let source = raw16(4096, 3072);
    let too_tall = geometry(area, rect(0, 0, 4096, 2306), SensorOrientation::Degrees90);
    assert!(matches!(
        check_geometry(&too_tall, &source),
        Err(ViewfinderError::InvalidGeometry { .. })
    ));
    let odd = geometry(area, rect(1, 0, 100, 100), SensorOrientation::Degrees90);
    assert!(check_geometry(&odd, &source).is_err());
    let odd_area = geometry(
        rect(0, 0, 4095, 2304),
        rect(0, 0, 100, 100),
        SensorOrientation::Degrees0,
    );
    assert!(check_geometry(&odd_area, &source).is_err());
    let area_outside_buffer = geometry(
        rect(0, 0, 4096, 3074),
        rect(0, 0, 100, 100),
        SensorOrientation::Degrees0,
    );
    assert!(check_geometry(&area_outside_buffer, &source).is_err());
    assert!(check_geometry(&geometry(area, area, SensorOrientation::Degrees90), &source).is_ok());
}

#[test]
fn raw10_buffer_must_hold_every_word_the_shader_reads() {
    // 16×4 пикселя: 4 группы по 5 байт = 20 байт строки, шаг 24.
    let area = rect(0, 0, 16, 4);
    let exact = geometry(area, area, SensorOrientation::Degrees0);
    // Последняя строка: 3·24 + 20 = 92 байта → округление до слова = 92.
    assert!(check_geometry(&exact, &raw10(16, 4, 24, 92)).is_ok());
    assert!(check_geometry(&exact, &raw10(16, 4, 24, 88)).is_err(), "буфер короче");
    assert!(
        check_geometry(&exact, &raw10(16, 4, 18, 1000)).is_err(),
        "шаг меньше строки"
    );
    // Кадрирование левой половины читает меньше байт строки.
    let left_half = geometry(area, rect(0, 0, 8, 4), SensorOrientation::Degrees0);
    let checked = check_geometry(&left_half, &raw10(16, 4, 24, 84)).unwrap();
    assert_eq!(checked.raw10_row_stride_bytes, 24);
}
