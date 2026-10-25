//! Расшифровка `AHardwareBuffer_Desc.format` и `usage` в читаемый вид (чистая логика).
//! Значения из `android/hardware_buffer.h` (NDK r30).

/// Все известные биты `usage` с именами. CPU_READ/WRITE — 4-битные поля, обрабатываются отдельно.
const USAGE_FLAGS: &[(u64, &str)] = &[
    (1 << 8, "GPU_SAMPLED_IMAGE"),
    (1 << 9, "GPU_FRAMEBUFFER"),
    (1 << 11, "COMPOSER_OVERLAY"),
    (1 << 14, "PROTECTED_CONTENT"),
    (1 << 16, "VIDEO_ENCODE"),
    (1 << 23, "SENSOR_DIRECT_DATA"),
    (1 << 24, "GPU_DATA_BUFFER"),
    (1 << 25, "GPU_CUBE_MAP"),
    (1 << 26, "GPU_MIPMAP_COMPLETE"),
    (1 << 28, "VENDOR_0"),
    (1 << 29, "VENDOR_1"),
    (1 << 30, "VENDOR_2"),
    (1 << 31, "VENDOR_3"),
    (1 << 32, "FRONT_BUFFER"),
];

/// `AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE` — без него (или GPU_DATA_BUFFER) Vulkan-импорт не заработает.
pub const USAGE_GPU_SAMPLED_IMAGE: u64 = 1 << 8;
/// `AHARDWAREBUFFER_USAGE_GPU_DATA_BUFFER`.
pub const USAGE_GPU_DATA_BUFFER: u64 = 1 << 24;

/// Имена всех установленных битов usage.
pub fn usage_flag_names(usage: u64) -> Vec<String> {
    let mut names = Vec::new();
    match usage & 0xF {
        0 => {}
        2 => names.push("CPU_READ_RARELY".to_owned()),
        3 => names.push("CPU_READ_OFTEN".to_owned()),
        other => names.push(format!("CPU_READ(0x{other:X})")),
    }
    match (usage >> 4) & 0xF {
        0 => {}
        2 => names.push("CPU_WRITE_RARELY".to_owned()),
        3 => names.push("CPU_WRITE_OFTEN".to_owned()),
        other => names.push(format!("CPU_WRITE(0x{other:X})")),
    }
    names.extend(
        USAGE_FLAGS
            .iter()
            .filter(|(bit, _)| usage & bit != 0)
            .map(|(_, name)| (*name).to_owned()),
    );
    names
}

/// Имя `AHARDWAREBUFFER_FORMAT_*`. Камерные RAW-форматы (0x20/0x24/0x25) не входят в
/// публичный enum AHB, но `AHardwareBuffer_describe` может вернуть их «как есть».
pub fn ahb_format_name(format: u32) -> String {
    match format {
        1 => "R8G8B8A8_UNORM".to_owned(),
        2 => "R8G8B8X8_UNORM".to_owned(),
        3 => "R8G8B8_UNORM".to_owned(),
        4 => "R5G6B5_UNORM".to_owned(),
        0x16 => "R16G16B16A16_FLOAT".to_owned(),
        0x20 => "(HAL) RAW16".to_owned(),
        0x21 => "BLOB".to_owned(),
        0x22 => "(HAL) IMPLEMENTATION_DEFINED".to_owned(),
        0x23 => "(HAL) YCbCr_420_888".to_owned(),
        0x24 => "(HAL) RAW_OPAQUE".to_owned(),
        0x25 => "(HAL) RAW10".to_owned(),
        0x26 => "(HAL) RAW12".to_owned(),
        0x2b => "R10G10B10A2_UNORM".to_owned(),
        0x38 => "R8_UNORM".to_owned(),
        0x39 => "R16_UINT".to_owned(),
        0x3a => "R16G16_UINT".to_owned(),
        0x3e => "R14_UINT".to_owned(),
        other => format!("UNKNOWN(0x{other:X})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_cpu_and_gpu_bits() {
        let usage = 0x3 | (0x2 << 4) | (1 << 8) | (1 << 24);
        assert_eq!(
            usage_flag_names(usage),
            [
                "CPU_READ_OFTEN",
                "CPU_WRITE_RARELY",
                "GPU_SAMPLED_IMAGE",
                "GPU_DATA_BUFFER"
            ]
        );
    }

    #[test]
    fn empty_usage_has_no_names() {
        assert!(usage_flag_names(0).is_empty());
    }

    #[test]
    fn format_names() {
        assert_eq!(ahb_format_name(0x39), "R16_UINT");
        assert_eq!(ahb_format_name(0x20), "(HAL) RAW16");
        assert_eq!(ahb_format_name(0xABC), "UNKNOWN(0xABC)");
    }
}
