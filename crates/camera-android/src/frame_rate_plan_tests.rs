use camera_contract::capabilities::FrameDuration;
use camera_contract::fixtures::{camera_id, front_camera, main_wide_camera, ultra_wide_camera};
use camera_contract::geometry::PixelSize;
use camera_contract::raw_format::RawFormat;
use camera_contract::stream::TargetFrameRate;

use super::*;
use crate::fake_metadata::{FRONT_TABLE, MAIN_TABLE, ULTRA_WIDE_TABLE};
use crate::sensor_modes::{SensorModeParseError, SensorModeProblem, SensorModeTable};
use crate::vendor::{TagResolution, TagResolver};

/// Устройство, знающее оба тега CamX (или не знающее ни одного).
struct Device {
    knows_camx: bool,
}

impl TagResolver for Device {
    fn resolve_tag(&self, _name: &str) -> TagResolution {
        if self.knows_camx {
            TagResolution::Found { tag: 0x804a_0000 }
        } else {
            TagResolution::NotFound
        }
    }
}

fn vendor_for(camera: &str, knows_camx: bool) -> VendorCapabilities {
    VendorCapabilities::resolve(
        &camera_id(camera),
        &AndroidCameraConfig::default(),
        &Device { knows_camx },
    )
}

fn stream(camera: &str, size: PixelSize, readout: ReadoutAspect, fps: u32) -> ResolvedStream {
    ResolvedStream {
        camera: camera_id(camera),
        raw_format: RawFormat::Raw10,
        buffer_size: size,
        readout,
        frame_rate: TargetFrameRate::new(fps).unwrap(),
    }
}

fn main_stream(readout: ReadoutAspect, fps: u32) -> ResolvedStream {
    stream("2", PixelSize::new(4096, 3072), readout, fps)
}

/// Таблица режимов, прочитанная из реального массива OnePlus 11.
fn listed(values: &[i32]) -> SensorModeAvailability {
    SensorModeAvailability::Listed(SensorModeTable::parse(values).unwrap())
}

fn plan_with_modes(
    stream: &ResolvedStream,
    capabilities: &CameraCapabilities,
    vendor: &VendorCapabilities,
    sensor_modes: &SensorModeAvailability,
) -> Result<FrameRateMechanism, UnsupportedStream> {
    plan_frame_rate(
        stream,
        capabilities,
        vendor,
        sensor_modes,
        &AndroidCameraConfig::default(),
    )
}

/// Планирование с таблицей основной камеры (для тестов, где таблица не должна влиять на исход).
fn plan(
    stream: &ResolvedStream,
    capabilities: &CameraCapabilities,
    vendor: &VendorCapabilities,
) -> Result<FrameRateMechanism, UnsupportedStream> {
    plan_with_modes(stream, capabilities, vendor, &listed(&MAIN_TABLE))
}

fn not_achievable(fps: u32) -> Result<FrameRateMechanism, UnsupportedStream> {
    Err(UnsupportedStream::FrameRateNotAchievable {
        requested: TargetFrameRate::new(fps).unwrap(),
    })
}

fn ae_range(fps: u32) -> Result<FrameRateMechanism, UnsupportedStream> {
    Ok(FrameRateMechanism::PublicAeTargetRange {
        range: FpsRange { min: fps, max: fps },
    })
}

fn front_stream(readout: ReadoutAspect, fps: u32) -> ResolvedStream {
    stream("1", PixelSize::new(2304, 1728), readout, fps)
}

#[test]
fn main_camera_16x9_at_60_uses_the_public_ae_range() {
    let main = main_wide_camera().capabilities;
    let mechanism = plan(
        &main_stream(ReadoutAspect::SixteenByNine, 60),
        &main,
        &vendor_for("2", true),
    )
    .unwrap();
    assert_eq!(
        mechanism,
        FrameRateMechanism::PublicAeTargetRange {
            range: FpsRange { min: 60, max: 60 }
        }
    );
}

#[test]
fn main_camera_4x3_at_60_uses_the_camx_override_with_exactly_16666667_ns() {
    let main = main_wide_camera().capabilities;
    let mechanism = plan(
        &main_stream(ReadoutAspect::FourByThree, 60),
        &main,
        &vendor_for("2", true),
    )
    .unwrap();
    assert_eq!(
        mechanism,
        FrameRateMechanism::VendorSensorFrameDurationOverride {
            frame_duration: FrameDuration::from_nanos(16_666_667)
        }
    );
}

#[test]
fn ultra_wide_4x3_at_60_uses_ae_range_never_camx_even_if_the_device_knows_the_tags() {
    let wide = ultra_wide_camera().capabilities;
    let ultra = stream("3", PixelSize::new(4000, 3000), ReadoutAspect::FourByThree, 60);
    let mechanism = plan_with_modes(&ultra, &wide, &vendor_for("3", true), &listed(&ULTRA_WIDE_TABLE)).unwrap();
    assert_eq!(
        mechanism,
        FrameRateMechanism::PublicAeTargetRange {
            range: FpsRange { min: 60, max: 60 }
        }
    );
}

#[test]
fn main_camera_4x3_at_60_without_camx_tags_is_not_achievable_instead_of_silently_switching_to_16x9() {
    let main = main_wide_camera().capabilities;
    let requested = TargetFrameRate::new(60).unwrap();
    assert_eq!(
        plan(
            &main_stream(ReadoutAspect::FourByThree, 60),
            &main,
            &vendor_for("2", false)
        ),
        Err(UnsupportedStream::FrameRateNotAchievable { requested })
    );
}

#[test]
fn thirty_fps_is_within_the_public_ceiling_and_picks_the_exact_declared_range() {
    let main = main_wide_camera().capabilities;
    // Объявлены [15,30] и [30,30]: для 30 нужен именно [30,30].
    for readout in [ReadoutAspect::FourByThree, ReadoutAspect::SixteenByNine] {
        let mechanism = plan(&main_stream(readout, 30), &main, &vendor_for("2", true)).unwrap();
        assert_eq!(
            mechanism,
            FrameRateMechanism::PublicAeTargetRange {
                range: FpsRange { min: 30, max: 30 }
            }
        );
    }
}

#[test]
fn within_ceiling_without_matching_declared_range_falls_back_to_hal_default() {
    let mut main = main_wide_camera().capabilities;
    main.ae_fps_ranges = vec![FpsRange { min: 30, max: 30 }];
    let mechanism = plan(
        &main_stream(ReadoutAspect::FourByThree, 24),
        &main,
        &vendor_for("2", true),
    )
    .unwrap();
    assert_eq!(mechanism, FrameRateMechanism::HalDefault);
    main.ae_fps_ranges.clear();
    let mechanism = plan(
        &main_stream(ReadoutAspect::FourByThree, 30),
        &main,
        &vendor_for("2", true),
    )
    .unwrap();
    assert_eq!(mechanism, FrameRateMechanism::HalDefault);
}

#[test]
fn narrowest_enclosing_range_wins_for_a_rate_between_declared_edges() {
    let ranges = [
        FpsRange { min: 15, max: 30 },
        FpsRange { min: 24, max: 30 },
        FpsRange { min: 30, max: 30 },
    ];
    assert_eq!(public_range_for(&ranges, 24), Some(FpsRange { min: 24, max: 30 }));
    assert_eq!(public_range_for(&ranges, 30), Some(FpsRange { min: 30, max: 30 }));
    assert_eq!(public_range_for(&ranges, 20), Some(FpsRange { min: 15, max: 30 }));
    assert_eq!(public_range_for(&ranges, 10), None);
}

#[test]
fn rate_above_the_configured_limit_is_not_achievable_without_a_vendor_lever() {
    let main = main_wide_camera().capabilities;
    let requested = TargetFrameRate::new(120).unwrap();
    assert_eq!(
        plan(
            &main_stream(ReadoutAspect::SixteenByNine, 120),
            &main,
            &vendor_for("2", true)
        ),
        Err(UnsupportedStream::FrameRateNotAchievable { requested })
    );
    // Предел — настройка: поднять его можно конфигом, а не правкой кода.
    let config = AndroidCameraConfig {
        max_public_range_frame_rate: 120,
        ..AndroidCameraConfig::default()
    };
    assert_eq!(
        plan_frame_rate(
            &main_stream(ReadoutAspect::SixteenByNine, 120),
            &main,
            &vendor_for("2", true),
            &listed(&MAIN_TABLE),
            &config
        ),
        Ok(FrameRateMechanism::PublicAeTargetRange {
            range: FpsRange { min: 120, max: 120 }
        })
    );
}

#[test]
fn buffer_size_the_camera_does_not_offer_is_format_not_offered() {
    let main = main_wide_camera().capabilities;
    let wrong_size = stream("2", PixelSize::new(4096, 2304), ReadoutAspect::SixteenByNine, 60);
    assert_eq!(
        plan(&wrong_size, &main, &vendor_for("2", true)),
        Err(UnsupportedStream::FormatNotOffered {
            format: RawFormat::Raw10
        })
    );
}

#[test]
fn zero_declared_frame_duration_means_unlimited_so_ae_range_is_used_for_any_rate() {
    let mut main = main_wide_camera().capabilities;
    for option in &mut main.raw_streams {
        option.min_frame_duration = FrameDuration::from_nanos(0);
    }
    main.ae_fps_ranges = vec![FpsRange { min: 60, max: 60 }];
    let mechanism = plan(
        &main_stream(ReadoutAspect::FourByThree, 60),
        &main,
        &vendor_for("2", true),
    )
    .unwrap();
    assert_eq!(
        mechanism,
        FrameRateMechanism::PublicAeTargetRange {
            range: FpsRange { min: 60, max: 60 }
        }
    );
}

/// Камера объявила минимальную длительность кадра RAW-потока (остальное — как у основной камеры).
fn plan_with_declared_min_duration(nanos: u64, fps: u32) -> FrameRateMechanism {
    let mut main = main_wide_camera().capabilities;
    for option in &mut main.raw_streams {
        option.min_frame_duration = FrameDuration::from_nanos(nanos);
    }
    main.ae_fps_ranges = vec![FpsRange { min: 60, max: 60 }];
    // Полный кадр 4:3: выше потолка это вендорский рычаг, в пределах потолка — публичный диапазон AE.
    plan(
        &main_stream(ReadoutAspect::FourByThree, fps),
        &main,
        &vendor_for("2", true),
    )
    .unwrap()
}

#[test]
fn declared_16666667_ns_means_60_fps_is_within_the_ceiling() {
    // Старое деление вниз давало 59 к/с и отправляло 4:3@60 в вендорский рычаг, хотя камера 60 к/с и так отдаёт.
    assert_eq!(
        plan_with_declared_min_duration(16_666_667, 60),
        FrameRateMechanism::PublicAeTargetRange {
            range: FpsRange { min: 60, max: 60 }
        }
    );
    assert_eq!(
        plan_with_declared_min_duration(16_666_666, 60),
        FrameRateMechanism::PublicAeTargetRange {
            range: FpsRange { min: 60, max: 60 }
        }
    );
}

#[test]
fn declared_33333333_ns_keeps_60_fps_above_the_ceiling() {
    assert_eq!(
        plan_with_declared_min_duration(33_333_333, 60),
        FrameRateMechanism::VendorSensorFrameDurationOverride {
            frame_duration: FrameDuration::from_nanos(16_666_667)
        }
    );
}

#[test]
fn duration_slightly_over_the_tolerance_is_above_the_ceiling() {
    let over = 16_666_667 + FRAME_DURATION_TOLERANCE_NANOS + 1;
    assert!(matches!(
        plan_with_declared_min_duration(over, 60),
        FrameRateMechanism::VendorSensorFrameDurationOverride { .. }
    ));
}

// --- Таблица режимов сенсора решает, можно ли обещать AE-диапазон выше публичного потолка ---

#[test]
fn front_4x3_at_60_is_not_achievable_because_the_table_has_no_60fps_mode() {
    // Регрессия приёмки 13: AE [60,60] на фронте уводил HAL в 4608x3456 и валил провайдер камеры.
    assert_eq!(
        plan_with_modes(
            &front_stream(ReadoutAspect::FourByThree, 60),
            &front_camera().capabilities,
            &vendor_for("1", true),
            &listed(&FRONT_TABLE)
        ),
        not_achievable(60)
    );
}

#[test]
fn front_16x9_at_60_is_not_achievable_because_2304x1296_exists_only_at_30() {
    assert_eq!(
        plan_with_modes(
            &front_stream(ReadoutAspect::SixteenByNine, 60),
            &front_camera().capabilities,
            &vendor_for("1", true),
            &listed(&FRONT_TABLE)
        ),
        not_achievable(60)
    );
}

#[test]
fn front_4x3_at_30_stays_on_the_public_range_without_consulting_the_table() {
    for sensor_modes in [SensorModeAvailability::AbsentOnDevice, listed(&FRONT_TABLE)] {
        assert_eq!(
            plan_with_modes(
                &front_stream(ReadoutAspect::FourByThree, 30),
                &front_camera().capabilities,
                &vendor_for("1", true),
                &sensor_modes
            ),
            ae_range(30)
        );
    }
}

#[test]
fn main_16x9_at_60_needs_the_table_to_offer_the_4096x2304_mode() {
    let main = main_wide_camera().capabilities;
    let stream = main_stream(ReadoutAspect::SixteenByNine, 60);
    // Вырезка 16:9 считается от ширины буфера: 4096 x 2304.
    assert_eq!(
        plan_with_modes(&stream, &main, &vendor_for("2", true), &listed(&MAIN_TABLE)),
        ae_range(60)
    );
    // Тот же запрос на таблице фронта (режима 4096x2304 нет) — отказ.
    assert_eq!(
        plan_with_modes(&stream, &main, &vendor_for("2", true), &listed(&FRONT_TABLE)),
        not_achievable(60)
    );
}

#[test]
fn main_4x3_at_60_keeps_the_camx_override_even_though_its_table_has_no_4096x3072_at_60() {
    let main = main_wide_camera().capabilities;
    for sensor_modes in [
        listed(&MAIN_TABLE),
        SensorModeAvailability::AbsentOnDevice,
        SensorModeAvailability::Unusable(SensorModeProblem::TagResolveFailed { status: 1 }),
    ] {
        assert_eq!(
            plan_with_modes(
                &main_stream(ReadoutAspect::FourByThree, 60),
                &main,
                &vendor_for("2", true),
                &sensor_modes
            ),
            Ok(FrameRateMechanism::VendorSensorFrameDurationOverride {
                frame_duration: FrameDuration::from_nanos(16_666_667)
            })
        );
    }
}

#[test]
fn without_a_usable_table_the_ae_branches_above_the_ceiling_are_not_achievable() {
    let ultra = ultra_wide_camera().capabilities;
    let ultra_stream = stream("3", PixelSize::new(4000, 3000), ReadoutAspect::FourByThree, 60);
    let main = main_wide_camera().capabilities;
    let main_16x9 = main_stream(ReadoutAspect::SixteenByNine, 60);
    let malformed = SensorModeAvailability::Unusable(SensorModeProblem::Malformed(
        SensorModeParseError::UnsupportedFieldCount { actual: 4 },
    ));
    for sensor_modes in [SensorModeAvailability::AbsentOnDevice, malformed] {
        assert_eq!(
            plan_with_modes(&ultra_stream, &ultra, &vendor_for("3", true), &sensor_modes),
            not_achievable(60)
        );
        assert_eq!(
            plan_with_modes(&main_16x9, &main, &vendor_for("2", true), &sensor_modes),
            not_achievable(60)
        );
    }
}

#[test]
fn a_widescreen_crop_that_does_not_fit_the_buffer_is_not_achievable_not_a_panic() {
    let mut main = main_wide_camera().capabilities;
    // Буфер 4096x2000: вырезка 4096x2304 в него не помещается.
    let narrow = PixelSize::new(4096, 2000);
    main.raw_streams[0].size = narrow;
    let stream = stream("2", narrow, ReadoutAspect::SixteenByNine, 60);
    assert_eq!(
        plan_with_modes(&stream, &main, &vendor_for("2", true), &listed(&MAIN_TABLE)),
        not_achievable(60)
    );
}
