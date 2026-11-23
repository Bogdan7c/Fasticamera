//! Перевод паспорта кадра в параметры предпросмотра `gpu-vulkan` и выбор раскладки источника.
//!
//! `gpu-vulkan` не знает про `camera-contract`; граница проходит здесь. Допущения предпросмотра
//! проверяются явно: неизвестная мозаика — ошибка, а не догадка.

use camera_contract::frame_passport::{FrameCaptureMeta, RawFramePassport};
use camera_contract::raw_format::{CfaLayout, MosaicKind};
use gpu_vulkan::raw_mosaic_preview::{CfaOrder, MosaicPreviewParams, QuarterTurns, ValidArea};

/// Паспорт нельзя показать предпросмотром.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PassportError {
    /// Мозаика не обычный Bayer 2×2 одной из четырёх раскладок.
    #[error("предпросмотр умеет только Bayer (RGGB/GRBG/GBRG/BGGR), а в паспорте {kind:?}/{layout:?}")]
    UnsupportedMosaic {
        /// Вид мозаики.
        kind: MosaicKind,
        /// Раскладка.
        layout: CfaLayout,
    },
    /// Ориентация сенсора не кратна четверти оборота (недостижимо для `SensorOrientation`, но не паникуем).
    #[error("ориентация сенсора вне четвертей оборота: {quarter_turns}")]
    BadOrientation {
        /// Число четвертей.
        quarter_turns: u32,
    },
}

/// Параметры предпросмотра по паспорту кадра.
///
/// Уровни: если результат запроса отдал динамические чёрный/белый уровни — берём их (точнее),
/// иначе статические из характеристик камеры.
pub fn preview_params(passport: &RawFramePassport) -> Result<MosaicPreviewParams, PassportError> {
    let stream = &passport.stream;
    if stream.mosaic.kind != MosaicKind::Bayer {
        return Err(PassportError::UnsupportedMosaic {
            kind: stream.mosaic.kind,
            layout: stream.mosaic.layout,
        });
    }
    let cfa = match stream.mosaic.layout {
        CfaLayout::Rggb => CfaOrder::Rggb,
        CfaLayout::Grbg => CfaOrder::Grbg,
        CfaLayout::Gbrg => CfaOrder::Gbrg,
        CfaLayout::Bggr => CfaOrder::Bggr,
        other => {
            return Err(PassportError::UnsupportedMosaic {
                kind: stream.mosaic.kind,
                layout: other,
            });
        }
    };
    let quarter_turns = stream.sensor_orientation.quarter_turns_clockwise();
    let sensor_rotation =
        QuarterTurns::from_count(quarter_turns).ok_or(PassportError::BadOrientation { quarter_turns })?;

    let dynamic = match &passport.capture {
        FrameCaptureMeta::Paired(details) => Some(details.color),
        FrameCaptureMeta::Missing { .. } => None,
    };
    let black_levels = dynamic
        .and_then(|color| color.dynamic_black_level)
        .map_or(stream.static_levels.black_level_pattern, |levels| {
            levels.map(round_level)
        });
    let white_level = dynamic
        .and_then(|color| color.dynamic_white_level)
        .unwrap_or(stream.static_levels.white_level);

    Ok(MosaicPreviewParams {
        valid_area: ValidArea {
            left: stream.valid_area.left,
            top: stream.valid_area.top,
            width: stream.valid_area.size.width,
            height: stream.valid_area.size.height,
        },
        cfa,
        black_levels,
        white_level,
        sensor_rotation,
    })
}

/// Уровень из `f32` метаданных в целое: ближайшее, не меньше нуля.
fn round_level(level: f32) -> u32 {
    // Приведение насыщающее: отрицательное и NaN дают 0, огромное — u32::MAX; уровни такого размера не бывают.
    level.round().max(0.0) as u32
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use camera_contract::capabilities::{LensShading, SensorLevels, VendorProcessingFlags};
    use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
    use camera_contract::fixtures::camera_id;
    use camera_contract::frame_passport::{
        CaptureColorHints, FrameCaptureDetails, FrameSequence, MissingCaptureMeta, StreamPassport,
    };
    use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
    use camera_contract::raw_format::{MosaicPattern, RawFormat};
    use camera_contract::stream::{FrameRateMechanism, ReadoutAspect, TargetFrameRate};

    use super::*;

    fn stream_passport(layout: CfaLayout, kind: MosaicKind) -> StreamPassport {
        StreamPassport {
            camera: camera_id("2"),
            raw_format: RawFormat::Raw10,
            buffer_size: PixelSize::new(4096, 3072),
            readout: ReadoutAspect::SixteenByNine,
            valid_area: PixelRect {
                left: 0,
                top: 0,
                size: PixelSize::new(4096, 2304),
            },
            sensor_area: camera_contract::lens_shading::ValidAreaOnSensor::whole_active_array(PixelSize::new(
                4096, 3072,
            )),
            lens_shading: camera_contract::lens_shading::LensShadingCalibrationState::NotCalibrated(
                camera_contract::lens_shading::NotCalibratedReason::NoCalibrationForCamera,
            ),
            mosaic: MosaicPattern { kind, layout },
            static_levels: SensorLevels {
                black_level_pattern: [64, 65, 66, 67],
                white_level: 1023,
            },
            vendor_processing: VendorProcessingFlags {
                lens_shading: LensShading::AppliedByVendor,
            },
            sensor_orientation: SensorOrientation::Degrees90,
            frame_rate: TargetFrameRate::new(60).expect("60 к/с допустимо"),
            frame_rate_mechanism: FrameRateMechanism::HalDefault,
            color_calibration: Vec::new(),
        }
    }

    fn passport_with(stream: StreamPassport, capture: FrameCaptureMeta) -> RawFramePassport {
        RawFramePassport {
            stream: Arc::new(stream),
            sequence: FrameSequence(5),
            sensor_timestamp: SensorTimestamp::from_nanos(77),
            capture,
        }
    }

    fn missing() -> FrameCaptureMeta {
        FrameCaptureMeta::Missing {
            reason: MissingCaptureMeta::PairingTimedOut,
        }
    }

    fn paired(color: CaptureColorHints) -> FrameCaptureMeta {
        FrameCaptureMeta::Paired(FrameCaptureDetails {
            result: CaptureResultMeta {
                frame_number: FrameNumber(1),
                sensor_timestamp: Some(SensorTimestamp::from_nanos(77)),
                exposure_time: None,
                sensitivity_iso: None,
                reported_frame_duration: None,
                rolling_shutter_skew: None,
                three_a: camera_contract::three_a::ThreeAResultState::unreported(
                    camera_contract::three_a::RequestOrigin::Repeating,
                ),
            },
            color,
        })
    }

    #[test]
    fn oneplus_style_passport_maps_area_cfa_levels_and_rotation() {
        let passport = passport_with(stream_passport(CfaLayout::Bggr, MosaicKind::Bayer), missing());
        let params = preview_params(&passport).expect("поддержанный паспорт");
        assert_eq!(params.cfa, CfaOrder::Bggr);
        assert_eq!(
            params.valid_area,
            ValidArea {
                left: 0,
                top: 0,
                width: 4096,
                height: 2304
            },
            "16:9 внутри буфера 4:3"
        );
        assert_eq!(params.black_levels, [64, 65, 66, 67]);
        assert_eq!(params.white_level, 1023);
        assert_eq!(params.sensor_rotation, QuarterTurns::Clockwise90);
    }

    #[test]
    fn dynamic_levels_win_over_static_ones_when_the_frame_has_them() {
        let color = CaptureColorHints {
            dynamic_black_level: Some([70.4, 71.6, 72.0, 73.2]),
            dynamic_white_level: Some(1000),
            ..CaptureColorHints::default()
        };
        let passport = passport_with(stream_passport(CfaLayout::Rggb, MosaicKind::Bayer), paired(color));
        let params = preview_params(&passport).expect("паспорт");
        assert_eq!(params.black_levels, [70, 72, 72, 73]);
        assert_eq!(params.white_level, 1000);

        let without_dynamic = passport_with(
            stream_passport(CfaLayout::Rggb, MosaicKind::Bayer),
            paired(CaptureColorHints::default()),
        );
        assert_eq!(
            preview_params(&without_dynamic).expect("паспорт").black_levels,
            [64, 65, 66, 67]
        );
    }

    #[test]
    fn unknown_mosaics_are_errors_not_guesses() {
        for (layout, kind) in [
            (CfaLayout::Mono, MosaicKind::Bayer),
            (CfaLayout::Rgb, MosaicKind::Bayer),
            (CfaLayout::Rggb, MosaicKind::QuadBayer),
            (CfaLayout::Rggb, MosaicKind::Rgbw),
        ] {
            let passport = passport_with(stream_passport(layout, kind), missing());
            assert!(
                matches!(preview_params(&passport), Err(PassportError::UnsupportedMosaic { .. })),
                "{kind:?}/{layout:?}"
            );
        }
    }
}
