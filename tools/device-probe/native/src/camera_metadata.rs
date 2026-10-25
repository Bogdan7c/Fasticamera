//! Чистый разбор «сырых» массивов Camera2 NDK metadata в структуры отчёта.
//!
//! Вход — уже прочитанные срезы из `ACameraMetadata_getConstEntry`; сам FFI живёт в
//! `camera_ndk`. Так логику (фильтр RAW, склейка с длительностью кадра, строки
//! physical id) можно проверить на host без телефона.

use std::collections::HashMap;

use crate::report::RawStreamConfig;

/// `AIMAGE_FORMAT_RAW16` (== `HAL_PIXEL_FORMAT_RAW16`).
pub const FORMAT_RAW16: i32 = 0x20;
/// `AIMAGE_FORMAT_RAW_PRIVATE`.
pub const FORMAT_RAW_PRIVATE: i32 = 0x24;
/// `AIMAGE_FORMAT_RAW10` (упакованный 10-битный).
pub const FORMAT_RAW10: i32 = 0x25;

/// Значение `isInput` для выходного потока в `SCALER_AVAILABLE_STREAM_CONFIGURATIONS`.
const STREAM_CONFIGURATION_OUTPUT: i32 = 0;

/// Является ли формат одним из интересующих нас RAW.
pub fn is_raw_format(format: i32) -> bool {
    matches!(format, FORMAT_RAW16 | FORMAT_RAW_PRIVATE | FORMAT_RAW10)
}

/// Читаемое имя формата для логов/отчёта.
pub fn format_name(format: i32) -> String {
    match format {
        FORMAT_RAW16 => "RAW16".to_owned(),
        FORMAT_RAW_PRIVATE => "RAW_PRIVATE".to_owned(),
        FORMAT_RAW10 => "RAW10".to_owned(),
        0x21 => "BLOB".to_owned(),
        0x22 => "PRIVATE".to_owned(),
        0x23 => "YUV_420_888".to_owned(),
        other => format!("0x{other:X}"),
    }
}

/// Ключ формата в таблице длительностей: (format, width, height).
type ModeKey = (i32, i32, i32);

/// Разбирает `ACAMERA_SCALER_AVAILABLE_MIN_FRAME_DURATIONS` (int64 × 4: format, w, h, ns).
///
/// Хвост, не кратный 4, отбрасывается: такой массив означает повреждённые данные,
/// но разведка не должна из-за этого падать.
fn index_min_durations(entries: &[i64]) -> HashMap<ModeKey, i64> {
    entries
        .chunks_exact(4)
        .filter_map(|quad| {
            let format = i32::try_from(quad[0]).ok()?;
            let width = i32::try_from(quad[1]).ok()?;
            let height = i32::try_from(quad[2]).ok()?;
            Some(((format, width, height), quad[3]))
        })
        .collect()
}

/// Собирает список выходных RAW-конфигураций с минимальной длительностью кадра.
///
/// * `stream_configs` — int32 × 4: format, w, h, isInput;
/// * `min_durations` — int64 × 4: format, w, h, ns.
pub fn build_raw_stream_configs(stream_configs: &[i32], min_durations: &[i64]) -> Vec<RawStreamConfig> {
    let durations = index_min_durations(min_durations);
    stream_configs
        .chunks_exact(4)
        .filter(|quad| is_raw_format(quad[0]) && quad[3] == STREAM_CONFIGURATION_OUTPUT)
        .map(|quad| RawStreamConfig {
            format: quad[0],
            format_name: format_name(quad[0]),
            width: quad[1],
            height: quad[2],
            min_frame_duration_ns: durations.get(&(quad[0], quad[1], quad[2])).copied(),
        })
        .collect()
}

/// Разбирает `ACAMERA_LOGICAL_MULTI_CAMERA_PHYSICAL_IDS`: байты, id разделены `\0`.
pub fn parse_physical_ids(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_output_raw_configs_and_joins_durations() {
        let stream_configs = [
            0x20, 4096, 3072, 0, // RAW16 output — оставить
            0x20, 4096, 3072, 1, // RAW16 input — выбросить
            0x23, 4000, 3000, 0, // YUV — выбросить
            0x25, 8192, 6144, 0, // RAW10 output, без длительности
        ];
        let durations = [0x20_i64, 4096, 3072, 33_333_333, 0x23, 4000, 3000, 1];
        let parsed = build_raw_stream_configs(&stream_configs, &durations);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].format, 0x20);
        assert_eq!(parsed[0].format_name, "RAW16");
        assert_eq!(parsed[0].min_frame_duration_ns, Some(33_333_333));
        assert_eq!(parsed[1].format, 0x25);
        assert_eq!((parsed[1].width, parsed[1].height), (8192, 6144));
        assert_eq!(parsed[1].min_frame_duration_ns, None);
    }

    #[test]
    fn truncated_arrays_do_not_panic() {
        assert!(build_raw_stream_configs(&[0x20, 1, 2], &[1, 2, 3]).is_empty());
    }

    #[test]
    fn physical_ids_split_on_nul() {
        assert_eq!(parse_physical_ids(b"2\x003\x00"), vec!["2", "3"]);
        assert_eq!(parse_physical_ids(b"\x00\x00"), Vec::<String>::new());
        assert_eq!(parse_physical_ids(b"10"), vec!["10"]);
    }
}
