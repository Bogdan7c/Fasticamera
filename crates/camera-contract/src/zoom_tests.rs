use super::*;
use crate::auto_exposure::RefreshSignal;
use crate::lens_calibration::UncalibratedReason;
use crate::three_a::SensorPoint;

fn ratio(value: f32) -> ZoomRatio {
    ZoomRatio::new(value).unwrap()
}

fn camera(id: &str) -> CameraId {
    CameraId::new(id).unwrap()
}

fn lens(role: LensRole, id: &str, native: f32, axis: OpticalAxis, mirror: PreviewMirror) -> LensZoomPoint {
    LensZoomPoint {
        role,
        camera: camera(id),
        native_ratio: ratio(native),
        equivalent_focal_mm: 23.6 * native,
        optical_axis: axis,
        active_array: PixelSize::new(4000, 3000),
        preview_mirror: mirror,
        max_digital_zoom: ZoomFactor::new(10.0).unwrap(),
    }
}

/// Шкала как у OnePlus 11: ширик 0,59 (ось смещена влево), основная 1 (без калибровки), фронт.
fn catalog() -> Arc<ZoomCatalog> {
    let axis = OpticalAxis::Calibrated {
        point: SensorPoint::new(0.44, 0.5).unwrap(),
    };
    let uncalibrated = OpticalAxis::Uncalibrated {
        reason: UncalibratedReason::NotReported,
    };
    let back = LensGroup {
        facing: LensFacing::Back,
        lenses: vec![
            lens(LensRole::UltraWide, "3", 0.59, axis, PreviewMirror::None),
            lens(LensRole::Wide, "2", 1.0, uncalibrated, PreviewMirror::None),
        ],
        stops: vec![],
        range: ZoomRange {
            min: ratio(0.59),
            max: ratio(10.0),
        },
    };
    let front = LensGroup {
        facing: LensFacing::Front,
        lenses: vec![lens(LensRole::Front, "1", 1.0, uncalibrated, PreviewMirror::Horizontal)],
        stops: vec![],
        range: ZoomRange {
            min: ratio(1.0),
            max: ratio(3.0),
        },
    };
    Arc::new(ZoomCatalog {
        back: Some(back),
        front: Some(front),
    })
}

fn framing(value: f32) -> ZoomFraming {
    ZoomFraming {
        aspect: CropAspect::FOUR_BY_THREE,
        facing: LensFacing::Back,
        ratio: ratio(value),
        catalog: catalog(),
    }
}

#[test]
fn ratio_rejects_zero_negative_and_nan() {
    for value in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert!(ZoomRatio::new(value).is_err(), "{value} принят");
    }
    assert_eq!(ZoomRatio::new(0.59).unwrap().value(), 0.59);
}

#[test]
fn ultra_wide_at_one_x_crops_by_native_ratio_on_its_optical_axis() {
    // 1× на ширике: кадрирование в 1/0,59 ≈ 1,695 раза, центр — на оптической оси (0,44 × 4000).
    let framed = framing(1.0).crop_for_camera(&camera("3"));
    assert_eq!(framed.fit, ZoomFit::Exact);
    assert!((framed.crop.zoom.value() - 1.0 / 0.59).abs() < 1e-5);
    assert_eq!(framed.crop.center, CropCenter::BufferPixel { x: 1760.0, y: 1500.0 });
    assert!((framed.shown_ratio.value() - 1.0).abs() < 1e-5);
    assert_eq!(framed.mirror, PreviewMirror::None);
}

#[test]
fn uncalibrated_lens_crops_around_area_center() {
    let framed = framing(2.0).crop_for_camera(&camera("2"));
    assert_eq!(framed.crop.center, CropCenter::ValidAreaCenter);
    assert_eq!(framed.crop.zoom.value(), 2.0);
    assert_eq!(framed.fit, ZoomFit::Exact);
}

#[test]
fn wider_than_lens_shows_lens_minimum_honestly() {
    // Переключение на ширик ещё идёт, а кадр — основной: показываем её минимум и говорим об этом.
    let framed = framing(0.7).crop_for_camera(&camera("2"));
    assert_eq!(framed.fit, ZoomFit::WiderThanLens);
    assert_eq!(framed.crop.zoom, ZoomFactor::NONE);
    assert_eq!(framed.shown_ratio.value(), 1.0);
}

#[test]
fn beyond_digital_limit_is_clamped_and_reported() {
    let framed = framing(25.0).crop_for_camera(&camera("2"));
    assert_eq!(framed.fit, ZoomFit::BeyondDigitalLimit);
    assert_eq!(framed.crop.zoom.value(), 10.0);
    assert_eq!(framed.shown_ratio.value(), 10.0);
}

#[test]
fn front_camera_is_mirrored() {
    let framed = framing(1.5).crop_for_camera(&camera("1"));
    assert_eq!(framed.mirror, PreviewMirror::Horizontal);
    assert_eq!(framed.crop.zoom.value(), 1.5);
}

#[test]
fn unknown_camera_gets_full_centered_frame() {
    // Логическая 0 (сценарии) в шкалу не входит: кадр целиком, без чужой оси и зеркала.
    let framed = framing(3.0).crop_for_camera(&camera("0"));
    assert_eq!(framed.fit, ZoomFit::CameraNotInCatalog);
    assert_eq!(
        framed.crop,
        ViewfinderCrop::centered(CropAspect::FOUR_BY_THREE, ZoomFactor::NONE)
    );
    assert_eq!(framed.mirror, PreviewMirror::None);
}

#[test]
fn catalog_finds_lens_by_camera_and_group_by_facing() {
    let catalog = catalog();
    let (group, lens) = catalog.lens_for_camera(&camera("1")).unwrap();
    assert_eq!(group.facing, LensFacing::Front);
    assert_eq!(lens.role, LensRole::Front);
    assert!(catalog.group(LensFacing::External).is_none());
    assert_eq!(catalog.group(LensFacing::Back).unwrap().lenses.len(), 2);
    assert_eq!(lens.max_ratio().value(), 10.0);
}

#[test]
fn range_clamp_keeps_ratio_inside() {
    let range = ZoomRange {
        min: ratio(0.59),
        max: ratio(10.0),
    };
    assert_eq!(range.clamp(ratio(0.3)).value(), 0.59);
    assert_eq!(range.clamp(ratio(30.0)).value(), 10.0);
    assert_eq!(range.clamp(ratio(1.4)).value(), 1.4);
}

#[derive(Default)]
struct CountingSignal(std::sync::atomic::AtomicU32);

impl RefreshSignal for CountingSignal {
    fn signal(&self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[test]
fn channel_keeps_latest_intent_and_wakes_bound_engine() {
    let channel = LensIntentChannel::new();
    // До привязки движка толчок пропускается, но значение лежит в ящике.
    let (_, delivery) = channel.publish(LensIntent {
        facing: LensFacing::Back,
        ratio: ratio(0.8),
        gesture: ZoomGesture::Continuous,
    });
    assert_eq!(delivery, SignalDelivery::NotBoundYet);
    let signal = Arc::new(CountingSignal::default());
    channel.refresh_signal().bind(signal.clone());
    // Привязка досылает один толчок за пропущенный.
    assert_eq!(signal.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    let (first, _) = channel.latest().unwrap();
    channel.publish(LensIntent {
        facing: LensFacing::Back,
        ratio: ratio(1.4),
        gesture: ZoomGesture::Discrete,
    });
    let (second, latest) = channel.newer_than(Some(first)).unwrap();
    assert!(second > first);
    assert_eq!(latest.ratio.value(), 1.4, "последнее намерение побеждает");
    assert!(channel.newer_than(Some(second)).is_none());
    assert_eq!(signal.0.load(std::sync::atomic::Ordering::SeqCst), 2);
}
