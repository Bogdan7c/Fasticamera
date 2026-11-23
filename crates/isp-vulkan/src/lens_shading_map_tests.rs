use isp_model::lens_shading::LensShadingSkipReason;

use super::*;

fn map(columns: u32, rows: u32) -> Arc<LensShadingGainMap> {
    // Каждая точка различима по номеру и каналу: перепутанный порядок байт или строк сразу виден.
    let points = (0..columns * rows)
        .map(|index| {
            let base = 1.0 + index as f32;
            [base, base + 0.25, base + 0.5, base + 0.75]
        })
        .collect();
    Arc::new(LensShadingGainMap::new(columns, rows, points).unwrap())
}

fn word(bytes: &[u8], index: usize) -> u32 {
    u32::from_ne_bytes([
        bytes[4 * index],
        bytes[4 * index + 1],
        bytes[4 * index + 2],
        bytes[4 * index + 3],
    ])
}

#[test]
fn applied_map_is_packed_as_header_then_points_row_by_row() {
    let step = LensShadingStep::Apply(map(3, 2));
    let bytes = pack_lens_shading_map(&step, PixelSize::new(64, 48), LensShadingMapLimits::default()).unwrap();
    assert_eq!(bytes.len(), 48 + 3 * 2 * 16);
    assert_eq!(
        [word(&bytes, 0), word(&bytes, 1), word(&bytes, 2), word(&bytes, 3)],
        [1, 3, 2, 0]
    );
    assert_eq!(
        [word(&bytes, 4), word(&bytes, 5), word(&bytes, 6), word(&bytes, 7)],
        [64, 48, 0, 0]
    );
    // Масштабы: (3 − 1)/(64 − 1) по X и (2 − 1)/(48 − 1) по Y — те же f32, что у CPU-эталона.
    assert_eq!(f32::from_bits(word(&bytes, 8)), 2.0_f32 / 63.0_f32);
    assert_eq!(f32::from_bits(word(&bytes, 9)), 1.0_f32 / 47.0_f32);
    // Точка (столбец 1, строка 1) — индекс 4 по строкам; значения всех четырёх каналов на своих местах.
    let first_word = 12 + 4 * 4;
    let point: Vec<f32> = (0..4)
        .map(|channel| f32::from_bits(word(&bytes, first_word + channel)))
        .collect();
    assert_eq!(point, vec![5.0, 5.25, 5.5, 5.75]);
}

#[test]
fn skipped_step_packs_a_disabled_stub_without_points() {
    for reason in [
        LensShadingSkipReason::NotCalibrated,
        LensShadingSkipReason::DisabledByControl,
    ] {
        let bytes = pack_lens_shading_map(
            &LensShadingStep::Skip(reason),
            PixelSize::new(64, 48),
            LensShadingMapLimits::default(),
        )
        .unwrap();
        assert_eq!(bytes.len(), 48, "{reason:?}: только заголовок");
        assert_eq!(word(&bytes, 0), 0, "{reason:?}: флаг «включено» сброшен");
    }
}

#[test]
fn map_larger_than_limits_is_an_explicit_error_not_a_silent_skip() {
    let limits = LensShadingMapLimits {
        max_columns: 4,
        max_rows: 4,
    };
    let area = PixelSize::new(64, 48);
    assert!(pack_lens_shading_map(&LensShadingStep::Apply(map(4, 4)), area, limits).is_ok());
    let too_wide = pack_lens_shading_map(&LensShadingStep::Apply(map(5, 2)), area, limits);
    assert_eq!(
        too_wide,
        Err(LensShadingMapError::TooLarge {
            columns: 5,
            rows: 2,
            max_columns: 4,
            max_rows: 4
        })
    );
    let too_tall = pack_lens_shading_map(&LensShadingStep::Apply(map(2, 5)), area, limits);
    assert!(matches!(too_tall, Err(LensShadingMapError::TooLarge { rows: 5, .. })));
    // Проверка без записи даёт тот же ответ.
    assert_eq!(
        check_lens_shading_step(&LensShadingStep::Apply(map(5, 2)), area, limits),
        too_wide.map(|_| ())
    );
}

#[test]
fn valid_area_smaller_than_two_by_two_is_rejected() {
    let step = LensShadingStep::Apply(map(2, 2));
    for size in [PixelSize::new(1, 48), PixelSize::new(64, 1), PixelSize::new(0, 0)] {
        assert_eq!(
            pack_lens_shading_map(&step, size, LensShadingMapLimits::default()),
            Err(LensShadingMapError::ValidAreaTooSmall { size })
        );
    }
}

#[test]
fn limits_below_two_by_two_are_invalid_and_default_buffer_size_is_documented() {
    let limits = LensShadingMapLimits::default();
    assert_eq!(limits.validate(), Ok(()));
    assert_eq!(limits.buffer_bytes(), 48 + 64 * 64 * 16);
    for broken in [
        LensShadingMapLimits {
            max_columns: 1,
            max_rows: 8,
        },
        LensShadingMapLimits {
            max_columns: 8,
            max_rows: 0,
        },
    ] {
        assert_eq!(
            broken.validate(),
            Err(LensShadingMapError::InvalidLimits { limits: broken })
        );
    }
}

#[test]
fn contents_match_only_the_same_map_instance_over_the_same_area() {
    let shared = map(3, 3);
    let area = PixelSize::new(64, 48);
    let stored = Contents::Map {
        map: Arc::clone(&shared),
        valid_area: area,
    };
    assert!(stored.matches(&LensShadingStep::Apply(Arc::clone(&shared)), area));
    // Другая область — карта «растянута» иначе: писать заново.
    assert!(!stored.matches(&LensShadingStep::Apply(Arc::clone(&shared)), PixelSize::new(64, 32)));
    // Равная по значению, но другая карта (другой Arc) — писать заново: сравнивать точки каждый кадр дороже записи.
    assert!(!stored.matches(&LensShadingStep::Apply(map(3, 3)), area));
    assert!(!stored.matches(&LensShadingStep::Skip(LensShadingSkipReason::NotCalibrated), area));
    assert!(Contents::Disabled.matches(&LensShadingStep::Skip(LensShadingSkipReason::DisabledByControl), area));
    assert!(!Contents::Disabled.matches(&LensShadingStep::Apply(shared), area));
    assert!(!Contents::Unwritten.matches(&LensShadingStep::Skip(LensShadingSkipReason::NotCalibrated), area));
}
