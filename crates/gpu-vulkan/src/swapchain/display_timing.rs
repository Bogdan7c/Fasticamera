//! `VK_GOOGLE_display_timing`: желаемое время показа, период обновления и журнал прошлых показов.
//!
//! Расширение есть на телефоне (профиль `AndroidDisplay`) и отсутствует на ПК-стенде. Отсутствие —
//! НЕ ошибка: `refresh_cycle` вернёт `None`, `drain_past_presentations` — пустой список, а в present
//! данные времени просто не добавляются. Время везде в `CLOCK_MONOTONIC` (наносекунды), как и тики vsync.

use std::time::Duration;

use ash::vk;
use shell_contract::clock::MonotonicNanos;

use super::{SwapchainError, swapchain_step};
use crate::context::GpuContext;
use crate::error::GpuError;

/// Один показ из журнала дисплея (`VkPastPresentationTimingGOOGLE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PastPresentation {
    /// Идентификатор показа, переданный в `present` (у нас — `frame_index`).
    pub present_id: u32,
    /// Желаемое время показа, если оно запрашивалось (`None` — запрошено «как можно раньше»).
    pub desired: Option<MonotonicNanos>,
    /// Когда кадр действительно показан.
    pub actual: MonotonicNanos,
    /// Самое раннее время, когда кадр мог быть показан.
    pub earliest: MonotonicNanos,
    /// Запас: на сколько раньше срока кадр был готов к показу. Чем меньше, тем ближе к пропуску.
    pub margin: Duration,
}

impl PastPresentation {
    /// Из записи драйвера. Времена > `i64::MAX` не бывают у реальных часов, но насыщаем, а не паникуем.
    pub(crate) fn from_vk(raw: &vk::PastPresentationTimingGOOGLE) -> Self {
        Self {
            present_id: raw.present_id,
            // 0 в журнале — «время не запрашивалось» (запрос присылал 0).
            desired: (raw.desired_present_time != 0).then(|| nanos_from_u64(raw.desired_present_time)),
            actual: nanos_from_u64(raw.actual_present_time),
            earliest: nanos_from_u64(raw.earliest_present_time),
            margin: Duration::from_nanos(raw.present_margin),
        }
    }
}

fn nanos_from_u64(value: u64) -> MonotonicNanos {
    MonotonicNanos::from_nanos(i64::try_from(value).unwrap_or(i64::MAX))
}

/// Запись `VkPresentTimeGOOGLE` для present. `None` → `0`: «без желаемого времени» (спецификация).
/// Отрицательное время (невозможное для CLOCK_MONOTONIC) тоже превращается в `0`.
pub(super) fn present_time_entry(present_id: u32, desired: Option<MonotonicNanos>) -> vk::PresentTimeGOOGLE {
    let desired_present_time = desired
        .map(|time| u64::try_from(time.as_nanos()).unwrap_or(0))
        .unwrap_or(0);
    vk::PresentTimeGOOGLE {
        present_id,
        desired_present_time,
    }
}

/// Включено ли расширение в контексте.
pub(super) fn is_supported(context: &GpuContext) -> bool {
    context.display_timing_loader().is_ok()
}

/// Период обновления дисплея; `None` — расширения нет или драйвер сообщил 0.
pub(super) fn refresh_cycle(
    context: &GpuContext,
    swapchain: vk::SwapchainKHR,
) -> Result<Option<Duration>, SwapchainError> {
    let loader = match context.display_timing_loader() {
        Ok(loader) => loader,
        Err(GpuError::ExtensionNotEnabled { .. }) => return Ok(None),
        Err(other) => return Err(other.into()),
    };
    // SAFETY: swapchain принадлежит цели и жив; расширение включено (loader получен).
    let cycle = swapchain_step(
        unsafe { loader.get_refresh_cycle_duration(swapchain) },
        "vkGetRefreshCycleDurationGOOGLE",
    )?;
    Ok((cycle.refresh_duration != 0).then(|| Duration::from_nanos(cycle.refresh_duration)))
}

/// Забирает накопленный журнал показов (драйвер удаляет выданные записи). Без расширения — пусто.
pub(super) fn drain_past_presentations(
    context: &GpuContext,
    swapchain: vk::SwapchainKHR,
) -> Result<Vec<PastPresentation>, SwapchainError> {
    let loader = match context.display_timing_loader() {
        Ok(loader) => loader,
        Err(GpuError::ExtensionNotEnabled { .. }) => return Ok(Vec::new()),
        Err(other) => return Err(other.into()),
    };
    // SAFETY: см. refresh_cycle.
    let raw = swapchain_step(
        unsafe { loader.get_past_presentation_timing(swapchain) },
        "vkGetPastPresentationTimingGOOGLE",
    )?;
    // Сырые записи — на уровне trace (по умолчанию отфильтрованы): по ним разбирают ритм показа
    // на телефоне, когда сводной статистики пейсинга мало (сессия 03).
    for record in &raw {
        tracing::trace!(
            target: "fasticamera::gpu::display_timing",
            present_id = record.present_id,
            desired_ns = record.desired_present_time,
            actual_ns = record.actual_present_time,
            earliest_ns = record.earliest_present_time,
            margin_ns = record.present_margin,
            "запись времени показа"
        );
    }
    Ok(raw.iter().map(PastPresentation::from_vk).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn past_presentation_converts_units_and_zero_desired_means_not_requested() {
        let raw = vk::PastPresentationTimingGOOGLE {
            present_id: 7,
            desired_present_time: 1_000,
            actual_present_time: 1_500,
            earliest_present_time: 900,
            present_margin: 250,
        };
        let converted = PastPresentation::from_vk(&raw);
        assert_eq!(converted.present_id, 7);
        assert_eq!(converted.desired, Some(MonotonicNanos::from_nanos(1_000)));
        assert_eq!(converted.actual, MonotonicNanos::from_nanos(1_500));
        assert_eq!(converted.earliest, MonotonicNanos::from_nanos(900));
        assert_eq!(converted.margin, Duration::from_nanos(250));

        let unrequested = PastPresentation::from_vk(&vk::PastPresentationTimingGOOGLE {
            desired_present_time: 0,
            ..raw
        });
        assert_eq!(unrequested.desired, None);
    }

    #[test]
    fn huge_driver_times_saturate_instead_of_panicking() {
        let raw = vk::PastPresentationTimingGOOGLE {
            actual_present_time: u64::MAX,
            ..Default::default()
        };
        assert_eq!(
            PastPresentation::from_vk(&raw).actual,
            MonotonicNanos::from_nanos(i64::MAX)
        );
    }

    #[test]
    fn present_time_entry_maps_optional_time_to_zero_or_nanoseconds() {
        let with_time = present_time_entry(3, Some(MonotonicNanos::from_nanos(42_000)));
        assert_eq!((with_time.present_id, with_time.desired_present_time), (3, 42_000));
        let without = present_time_entry(4, None);
        assert_eq!((without.present_id, without.desired_present_time), (4, 0));
        let negative = present_time_entry(5, Some(MonotonicNanos::from_nanos(-1)));
        assert_eq!(negative.desired_present_time, 0);
    }
}
