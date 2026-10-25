//! Чистая логика теста «Vulkan → входная Surface MediaCodec» (собирается и тестируется на host):
//! проверка параметров, политика выбора формата swapchain, цвет кадра, время показа, медиана.
//!
//! Форматы и цветовые пространства здесь хранятся как «сырые» числа Vulkan (`VkFormat`,
//! `VkColorSpaceKHR`), чтобы модуль не зависел от `ash` (его нет на host-сборке).

/// `VK_FORMAT_R8G8B8A8_UNORM`.
pub const FORMAT_R8G8B8A8_UNORM: i32 = 37;
/// `VK_FORMAT_A2R10G10B10_UNORM_PACK32`.
pub const FORMAT_A2R10G10B10_UNORM: i32 = 58;
/// `VK_FORMAT_A2B10G10R10_UNORM_PACK32`.
pub const FORMAT_A2B10G10R10_UNORM: i32 = 64;
/// `VK_COLOR_SPACE_SRGB_NONLINEAR_KHR`.
pub const COLOR_SPACE_SRGB_NONLINEAR: i32 = 0;
/// `VK_COLOR_SPACE_HDR10_HLG_EXT`.
pub const COLOR_SPACE_HDR10_HLG: i32 = 1_000_104_003;
/// `VK_COLOR_SPACE_HDR10_ST2084_EXT`.
pub const COLOR_SPACE_HDR10_ST2084: i32 = 1_000_104_008;

/// Пара «формат + цветовое пространство» в сыром виде.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawSurfaceFormat {
    pub format: i32,
    pub color_space: i32,
}

/// Проверенные параметры теста (приходят из Kotlin как знаковые числа).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderParams {
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    /// Интервал между кадрами, нс. 0 — без паузы между кадрами.
    pub frame_interval_ns: u64,
    pub prefer_ten_bit: bool,
}

impl RenderParams {
    /// Переводит знаковые JNI-значения в беззнаковые, отвергая бессмысленные.
    pub fn from_jni(
        width: i32,
        height: i32,
        frames: i32,
        frame_interval_ns: i64,
        prefer_ten_bit: bool,
    ) -> Result<Self, String> {
        let positive = |value: i32, name: &str| {
            u32::try_from(value)
                .ok()
                .filter(|v| *v > 0)
                .ok_or(format!("{name} должен быть > 0, получено {value}"))
        };
        Ok(Self {
            width: positive(width, "width")?,
            height: positive(height, "height")?,
            frames: u32::try_from(frames).map_err(|_| format!("frames должен быть >= 0, получено {frames}"))?,
            frame_interval_ns: u64::try_from(frame_interval_ns)
                .map_err(|_| format!("frameIntervalNs должен быть >= 0, получено {frame_interval_ns}"))?,
            prefer_ten_bit,
        })
    }
}

/// Является ли формат 10-битным (A2…10G10R10 UNORM).
pub fn is_ten_bit_format(format: i32) -> bool {
    matches!(format, FORMAT_A2R10G10B10_UNORM | FORMAT_A2B10G10R10_UNORM)
}

/// Ранг пары (меньше — лучше). Порядок: 10-бит (если просили) с sRGB → HDR10 PQ → HLG →
/// любое 10-бит; затем RGBA8 с sRGB → любой RGBA8; остальное — по порядку драйвера.
fn rank(candidate: &RawSurfaceFormat, prefer_ten_bit: bool) -> u8 {
    let ten_bit_rank = match candidate.color_space {
        COLOR_SPACE_SRGB_NONLINEAR => 0,
        COLOR_SPACE_HDR10_ST2084 => 1,
        COLOR_SPACE_HDR10_HLG => 2,
        _ => 3,
    };
    if prefer_ten_bit && is_ten_bit_format(candidate.format) {
        return ten_bit_rank;
    }
    match (
        candidate.format == FORMAT_R8G8B8A8_UNORM,
        candidate.color_space == COLOR_SPACE_SRGB_NONLINEAR,
    ) {
        (true, true) => 4,
        (true, false) => 5,
        _ => 6,
    }
}

/// Выбирает формат swapchain; `None` только если список пуст. При равных рангах — первый.
pub fn select_surface_format(available: &[RawSurfaceFormat], prefer_ten_bit: bool) -> Option<RawSurfaceFormat> {
    available
        .iter()
        .copied()
        .min_by_key(|candidate| rank(candidate, prefer_ten_bit))
}

/// Число картинок swapchain: не меньше 3 и не меньше `min`, не больше `max` (0 = без предела).
pub fn choose_image_count(min_count: u32, max_count: u32) -> u32 {
    let wanted = min_count.max(3);
    if max_count == 0 { wanted } else { wanted.min(max_count) }
}

/// Цвет очистки кадра: меняется каждый кадр, чтобы энкодер получал разные картинки.
pub fn frame_clear_color(frame_index: u32) -> [f32; 4] {
    let phase = (frame_index % 60) as f32 / 60.0;
    [phase, 1.0 - phase, 0.5, 1.0]
}

/// Желаемое время показа кадра (нс): `base + index * interval`, без переполнения.
pub fn desired_present_time_ns(base_ns: u64, frame_index: u32, frame_interval_ns: u64) -> u64 {
    base_ns.saturating_add(u64::from(frame_index).saturating_mul(frame_interval_ns))
}

/// Медиана (для чётного числа — верхняя из двух средних); пустой набор → 0.
pub fn median_u64(values: &[u64]) -> u64 {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted.get(sorted.len() / 2).copied().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn pair(format: i32, color_space: i32) -> RawSurfaceFormat {
        RawSurfaceFormat { format, color_space }
    }

    #[test]
    fn ten_bit_preferred_when_available() {
        let list = [pair(FORMAT_R8G8B8A8_UNORM, 0), pair(FORMAT_A2B10G10R10_UNORM, 0)];
        assert_eq!(
            select_surface_format(&list, true),
            Some(pair(FORMAT_A2B10G10R10_UNORM, 0))
        );
    }

    #[test]
    fn ten_bit_ignored_when_not_requested() {
        let list = [pair(FORMAT_A2B10G10R10_UNORM, 0), pair(FORMAT_R8G8B8A8_UNORM, 0)];
        assert_eq!(
            select_surface_format(&list, false),
            Some(pair(FORMAT_R8G8B8A8_UNORM, 0))
        );
    }

    #[test]
    fn falls_back_to_rgba8_without_ten_bit() {
        let list = [pair(44, 0), pair(FORMAT_R8G8B8A8_UNORM, 0)];
        assert_eq!(select_surface_format(&list, true), Some(pair(FORMAT_R8G8B8A8_UNORM, 0)));
    }

    #[test]
    fn falls_back_to_first_when_nothing_known() {
        let list = [pair(44, 0), pair(50, 0)];
        assert_eq!(select_surface_format(&list, true), Some(pair(44, 0)));
    }

    #[test]
    fn ten_bit_color_space_order_srgb_then_pq_then_hlg() {
        let list = [
            pair(FORMAT_A2B10G10R10_UNORM, COLOR_SPACE_HDR10_HLG),
            pair(FORMAT_A2B10G10R10_UNORM, COLOR_SPACE_HDR10_ST2084),
        ];
        assert_eq!(select_surface_format(&list, true), Some(list[1]));
        let with_srgb = [list[0], list[1], pair(FORMAT_A2R10G10B10_UNORM, 0)];
        assert_eq!(select_surface_format(&with_srgb, true), Some(with_srgb[2]));
    }

    #[test]
    fn empty_list_gives_none() {
        assert_eq!(select_surface_format(&[], true), None);
    }

    #[test]
    fn image_count_respects_minimum_three_and_limits() {
        assert_eq!(choose_image_count(2, 0), 3);
        assert_eq!(choose_image_count(5, 0), 5);
        assert_eq!(choose_image_count(2, 8), 3);
        assert_eq!(choose_image_count(2, 2), 2);
    }

    #[test]
    fn params_reject_nonsense() {
        assert!(RenderParams::from_jni(0, 2160, 10, 0, false).is_err());
        assert!(RenderParams::from_jni(3840, 2160, -1, 0, false).is_err());
        assert!(RenderParams::from_jni(3840, 2160, 10, -5, false).is_err());
        let ok = RenderParams::from_jni(3840, 2160, 600, 16_666_667, true).expect("валидные параметры");
        assert_eq!(
            (ok.width, ok.height, ok.frames, ok.frame_interval_ns),
            (3840, 2160, 600, 16_666_667)
        );
    }

    #[test]
    fn present_time_is_monotonic_and_saturates() {
        assert_eq!(desired_present_time_ns(1000, 0, 10), 1000);
        assert_eq!(desired_present_time_ns(1000, 3, 10), 1030);
        assert_eq!(desired_present_time_ns(u64::MAX - 1, 5, 10), u64::MAX);
    }

    #[test]
    fn clear_color_changes_between_frames() {
        assert_ne!(frame_clear_color(0), frame_clear_color(1));
        assert_eq!(frame_clear_color(0), frame_clear_color(60));
    }

    #[test]
    fn median_works() {
        assert_eq!(median_u64(&[]), 0);
        assert_eq!(median_u64(&[9, 1, 5]), 5);
        assert_eq!(median_u64(&[4, 1, 2, 3]), 3);
    }
}
