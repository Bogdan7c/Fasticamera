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
        standby: StandbyDemand::NotNeeded,
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
        standby: StandbyDemand::NotNeeded,
    });
    let (second, latest) = channel.newer_than(Some(first)).unwrap();
    assert!(second > first);
    assert_eq!(latest.ratio.value(), 1.4, "последнее намерение побеждает");
    assert!(channel.newer_than(Some(second)).is_none());
    assert_eq!(signal.0.load(std::sync::atomic::Ordering::SeqCst), 2);
}

/// Где оптическая ось камеры оказывается внутри её разрешённого кадрирования, в долях кадра (0,5 — центр).
fn axis_in_crop(lens: &LensZoomPoint, framed: &FramedCrop, valid: crate::geometry::PixelRect) -> (f64, f64) {
    let OpticalAxis::Calibrated { point } = lens.optical_axis else {
        panic!("в тесте выравнивания ось откалибрована");
    };
    let rect = framed.crop.resolve(valid).unwrap();
    let axis_x = f64::from(point.x()) * f64::from(lens.active_array.width);
    let axis_y = f64::from(point.y()) * f64::from(lens.active_array.height);
    (
        (axis_x - f64::from(rect.left)) / f64::from(rect.size.width),
        (axis_y - f64::from(rect.top)) / f64::from(rect.size.height),
    )
}

fn calibrated_lens(role: LensRole, id: &str, native: f32, active: PixelSize, axis_px: (f32, f32)) -> LensZoomPoint {
    LensZoomPoint {
        optical_axis: OpticalAxis::Calibrated {
            point: SensorPoint::new(axis_px.0 / active.width as f32, axis_px.1 / active.height as f32).unwrap(),
        },
        active_array: active,
        ..lens(
            role,
            id,
            native,
            OpticalAxis::Calibrated {
                point: SensorPoint::CENTER,
            },
            PreviewMirror::None,
        )
    }
}

#[test]
fn neighbouring_lenses_put_their_optical_axis_at_the_same_screen_point_after_a_switch() {
    // Синтетические калибровки «как у OnePlus 11»: ось основной почти в центре, ось ширика левее на 6 %.
    // Далёкая точка на оптической оси попадает в центр кадра и на основной, и на ширике при одном и том же
    // коэффициенте шкалы — картинка при смене камеры не прыгает (параллакс близких объектов — вне модели).
    let main_active = PixelSize::new(4096, 3072);
    let wide_active = PixelSize::new(4000, 3000);
    let main = calibrated_lens(LensRole::Wide, "2", 1.0, main_active, (2040.3, 1515.4));
    let ultra = calibrated_lens(LensRole::UltraWide, "3", 0.593, wide_active, (1759.3, 1494.8));
    let back = LensGroup {
        facing: LensFacing::Back,
        lenses: vec![ultra.clone(), main.clone()],
        stops: vec![],
        range: ZoomRange {
            min: ratio(0.593),
            max: ratio(10.0),
        },
    };
    let catalog = Arc::new(ZoomCatalog {
        back: Some(back),
        front: None,
    });
    let valid = |size: PixelSize| crate::geometry::PixelRect { left: 0, top: 0, size };
    for scale_ratio in [1.0_f32, 1.4, 2.0] {
        let framing = ZoomFraming {
            aspect: CropAspect::FOUR_BY_THREE,
            facing: LensFacing::Back,
            ratio: ratio(scale_ratio),
            catalog: Arc::clone(&catalog),
        };
        let on_main = axis_in_crop(&main, &framing.crop_for_camera(&camera("2")), valid(main_active));
        let on_ultra = axis_in_crop(&ultra, &framing.crop_for_camera(&camera("3")), valid(wide_active));
        // Главное требование «не прыгает»: точка оси на экране у обеих камер одна и та же. Допуск 1 % кадра —
        // на 1× основная показывает весь сенсор и не может сдвинуть кадр к своей оси (8 и 21 px от центра,
        // ≈ 0,7 %); это остаток, который видно при смене камеры.
        assert!(
            (on_main.0 - on_ultra.0).abs() < 0.01 && (on_main.1 - on_ultra.1).abs() < 0.01,
            "при {scale_ratio}×: ось на основной в {on_main:?}, на ширике в {on_ultra:?}"
        );
        // Когда обе камеры кадрированы с запасом, ось — ровно в центре (допуск — чётное выравнивание краёв).
        if scale_ratio >= 1.4 {
            for (axis, name) in [(on_main, "основная"), (on_ultra, "ширик")] {
                assert!(
                    (axis.0 - 0.5).abs() < 2e-3 && (axis.1 - 0.5).abs() < 2e-3,
                    "{name} при {scale_ratio}×: ось в {axis:?}, а не в центре кадра"
                );
            }
        }
    }
}

#[test]
fn ultra_wide_near_its_native_ratio_cannot_center_its_axis_and_shifts_toward_it() {
    // Ниже ≈ 0,674× (ширик кадрирован меньше чем в 1,137 раза) кадр ширика не помещается центром на ось:
    // кадрирование упирается в левый край сенсора, и картинка при зуме 0,6 → 0,67 «съезжает» к оси.
    // Это честное следствие геометрии (заметка для 13a), а не ошибка формулы.
    let wide_active = PixelSize::new(4000, 3000);
    let ultra = calibrated_lens(LensRole::UltraWide, "3", 0.593, wide_active, (1759.3, 1494.8));
    let catalog = Arc::new(ZoomCatalog {
        back: Some(LensGroup {
            facing: LensFacing::Back,
            lenses: vec![ultra.clone()],
            stops: vec![],
            range: ZoomRange {
                min: ratio(0.593),
                max: ratio(10.0),
            },
        }),
        front: None,
    });
    let valid = crate::geometry::PixelRect {
        left: 0,
        top: 0,
        size: wide_active,
    };
    let framed_at = |value: f32| {
        ZoomFraming {
            aspect: CropAspect::FOUR_BY_THREE,
            facing: LensFacing::Back,
            ratio: ratio(value),
            catalog: Arc::clone(&catalog),
        }
        .crop_for_camera(&camera("3"))
    };
    assert_eq!(framed_at(0.62).crop.resolve(valid).unwrap().left, 0, "у края сенсора");
    let centered = axis_in_crop(&ultra, &framed_at(0.70), valid);
    assert!(
        (centered.0 - 0.5).abs() < 2e-3,
        "с 0,70× ось уже в центре: {centered:?}"
    );
}

#[test]
fn intent_constructors_name_the_standby_demand_and_serialize_it() {
    let plain = LensIntent::without_standby(LensFacing::Back, ZoomRatio::ONE, ZoomGesture::Discrete);
    assert_eq!(plain.standby, StandbyDemand::NotNeeded);
    let warm = plain.with_standby(StandbyDemand::KeepNeighbourWarm);
    assert_eq!(warm.standby, StandbyDemand::KeepNeighbourWarm);
    assert_eq!(
        (warm.facing, warm.gesture),
        (plain.facing, plain.gesture),
        "прочее не меняется"
    );
    let json = serde_json::to_value(warm).expect("сериализация");
    assert_eq!(json["standby"], "keep_neighbour_warm");
    assert_eq!(StandbyDemand::default(), StandbyDemand::NotNeeded);
}
