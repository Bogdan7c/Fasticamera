//! Тесты validation: чистая логика счётчиков и живой хук на реальном GPU.

use ash::vk;

use super::*;
use crate::context::test_support::{gpu_context_or_skip, gpu_context_with_validation_or_skip};

#[test]
fn counters_split_errors_warnings_and_remember_first_error() {
    let counters = ValidationCounters::default();
    counters.record(vk::DebugUtilsMessageSeverityFlagsEXT::WARNING, "", "предупреждение");
    counters.record(vk::DebugUtilsMessageSeverityFlagsEXT::INFO, "", "инфо не считается");
    counters.record(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR, "", "первая");
    counters.record(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR, "", "вторая");
    let report = counters.report();
    assert_eq!(report.errors, 2);
    assert_eq!(report.warnings, 1);
    assert_eq!(report.first_error.as_deref(), Some("первая"));
}

#[test]
fn mode_helpers_describe_requested_validation() {
    assert!(!ValidationMode::Disabled.is_enabled());
    assert!(ValidationMode::FULL.is_enabled());
    assert!(ValidationMode::FULL.wants_synchronization());
    let basic = ValidationMode::Enabled {
        synchronization: SyncValidation::Disabled,
        known_deviations: KnownValidationDeviations::none(),
    };
    assert!(basic.is_enabled() && !basic.wants_synchronization());
}

/// Доказывает, что хук живой: заведомо неверный вызов даёт `errors > 0`.
/// Без этого теста «0 ошибок validation» в остальных GPU-тестах ничего бы не значило.
#[test]
fn deliberate_invalid_usage_is_reported_by_validation_hook() {
    let Some(context) = gpu_context_or_skip() else { return };
    assert_eq!(
        context.validation_report().errors,
        0,
        "до провокации ошибок быть не должно"
    );

    // Буфер нулевого размера нарушает VUID-VkBufferCreateInfo-size-00912.
    let invalid_buffer_info = vk::BufferCreateInfo::default()
        .size(0)
        .usage(vk::BufferUsageFlags::TRANSFER_DST)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    // SAFETY: устройство валидно; вызов намеренно неверен — это и есть проверка хука.
    // Результат драйвера не важен: если буфер создался, уничтожаем его.
    let created = unsafe { context.device().create_buffer(&invalid_buffer_info, None) };
    if let Ok(buffer) = created {
        // SAFETY: буфер только что создан этим устройством и нигде не используется.
        unsafe { context.device().destroy_buffer(buffer, None) };
    }

    let report = context.validation_report();
    assert!(report.errors > 0, "validation не поймала заведомую ошибку: {report:?}");
    assert!(
        report.first_error.as_deref().is_some_and(|text| text.contains("size")),
        "первое сообщение должно быть про size: {report:?}"
    );
}

/// Доказывает, что включена именно СИНХРОНИЗАЦИОННАЯ валидация: два заполнения одного буфера
/// без барьера между ними — гонка запись-после-записи, которую обычный слой не видит.
#[test]
fn sync_validation_catches_missing_barrier_between_two_writes() {
    use crate::context::MemoryLocation;
    use crate::live_objects::OwnedBuffer;

    let Some(context) = gpu_context_or_skip() else { return };
    let buffer_info = vk::BufferCreateInfo::default()
        .size(256)
        .usage(vk::BufferUsageFlags::TRANSFER_DST)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    // SAFETY: устройство валидно, параметры корректны.
    let buffer = OwnedBuffer::from_raw(
        &context,
        unsafe { context.device().create_buffer(&buffer_info, None) }.unwrap(),
    );
    // SAFETY: буфер только что создан.
    let requirements = unsafe { context.device().get_buffer_memory_requirements(buffer.handle()) };
    let memory = context
        .allocate_memory(requirements, MemoryLocation::DeviceLocal, "тест гонки")
        .unwrap();
    // SAFETY: память подходит и ещё не привязана.
    unsafe { context.device().bind_buffer_memory(buffer.handle(), memory.handle(), 0) }.unwrap();

    context
        .run_one_shot(|command_buffer| {
            // SAFETY: буфер записи в состоянии записи; два заполнения одного диапазона БЕЗ барьера — намеренно.
            unsafe {
                context
                    .device()
                    .cmd_fill_buffer(command_buffer, buffer.handle(), 0, 256, 1);
                context
                    .device()
                    .cmd_fill_buffer(command_buffer, buffer.handle(), 0, 256, 2);
            }
            Ok(())
        })
        .unwrap();
    context.wait_idle().unwrap();

    let report = context.validation_report();
    assert!(
        report
            .first_error
            .as_deref()
            .is_some_and(|text| text.contains("SYNC-HAZARD-WRITE-AFTER-WRITE")),
        "sync-валидация не поймала гонку: {report:?}"
    );
}

/// VUID, который слой выдаёт на буфер нулевого размера (см. тест выше).
const ZERO_SIZE_BUFFER_VUID: &str = "VUID-VkBufferCreateInfo-size-00912";

fn deviations_for(vuid: &str) -> KnownValidationDeviations {
    KnownValidationDeviations::new(vec![KnownValidationDeviation {
        vuid: vuid.to_owned(),
        reason: "тестовая причина".to_owned(),
    }])
}

/// Намеренно неверный вызов: буфер нулевого размера.
fn trigger_zero_size_buffer_error(context: &crate::context::GpuContext) {
    let info = vk::BufferCreateInfo::default()
        .size(0)
        .usage(vk::BufferUsageFlags::TRANSFER_DST)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    // SAFETY: вызов намеренно неверен; созданный драйвером буфер (если есть) сразу уничтожаем.
    if let Ok(buffer) = unsafe { context.device().create_buffer(&info, None) } {
        // SAFETY: буфер создан выше этим устройством.
        unsafe { context.device().destroy_buffer(buffer, None) };
    }
}

#[test]
fn listed_vuid_counts_as_known_deviation_not_error() {
    let mode = ValidationMode::full_with_known_deviations(deviations_for(ZERO_SIZE_BUFFER_VUID));
    let Some(context) = gpu_context_with_validation_or_skip(mode) else {
        return;
    };
    trigger_zero_size_buffer_error(&context);
    let report = context.validation_report();
    assert_eq!(report.errors, 0, "перечисленный VUID не ошибка: {report:?}");
    assert_eq!(report.first_error, None);
    assert_eq!(
        report.known_deviations.get(ZERO_SIZE_BUFFER_VUID),
        Some(&1),
        "{report:?}"
    );
    assert_eq!(report.known_deviation_total(), 1);
}

#[test]
fn unlisted_vuid_still_counts_as_error() {
    let mode = ValidationMode::full_with_known_deviations(deviations_for("VUID-VkMemoryAllocateInfo-pNext-02387"));
    let Some(context) = gpu_context_with_validation_or_skip(mode) else {
        return;
    };
    trigger_zero_size_buffer_error(&context);
    let report = context.validation_report();
    assert!(report.errors > 0, "{report:?}");
    // Перечисленный, но не сработавший VUID присутствует в отчёте с нулём.
    assert_eq!(
        report.known_deviations.get("VUID-VkMemoryAllocateInfo-pNext-02387"),
        Some(&0)
    );
}

#[test]
fn vuid_matching_is_exact_not_prefix() {
    // Префикс настоящего VUID не должен «гасить» ошибку.
    let prefix = &ZERO_SIZE_BUFFER_VUID[..ZERO_SIZE_BUFFER_VUID.len() - 2];
    let counters = ValidationCounters::new(deviations_for(prefix));
    let recorded = counters.record(
        vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
        ZERO_SIZE_BUFFER_VUID,
        "текст",
    );
    assert_eq!(recorded, RecordedMessage::Error);
    let report = counters.report();
    assert_eq!(report.errors, 1);
    assert_eq!(report.known_deviations.get(prefix), Some(&0));
}

#[test]
fn known_deviation_reports_its_reason_and_skips_first_error() {
    let counters = ValidationCounters::new(deviations_for(ZERO_SIZE_BUFFER_VUID));
    let recorded = counters.record(
        vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
        ZERO_SIZE_BUFFER_VUID,
        "текст",
    );
    assert_eq!(
        recorded,
        RecordedMessage::KnownDeviation {
            reason: "тестовая причина"
        }
    );
    assert_eq!(counters.report().first_error, None);
}

#[test]
fn known_deviations_since_reports_only_new_hits() {
    let counters = ValidationCounters::new(deviations_for(ZERO_SIZE_BUFFER_VUID));
    let error = vk::DebugUtilsMessageSeverityFlagsEXT::ERROR;
    counters.record(error, ZERO_SIZE_BUFFER_VUID, "до");
    let before = counters.report();
    counters.record(error, ZERO_SIZE_BUFFER_VUID, "после 1");
    counters.record(error, ZERO_SIZE_BUFFER_VUID, "после 2");
    let delta = counters.report().known_deviations_since(&before);
    assert_eq!(delta.get(ZERO_SIZE_BUFFER_VUID), Some(&2));
}
