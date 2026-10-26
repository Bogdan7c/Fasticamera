//! Чистые тесты выбора устройства: подставляем отчёты вместо драйвера.

use super::*;

const FAKE_MISSING_EXTENSION: &CStr = c"VK_FAKE_never_exists";

fn full_features() -> FeatureSupport {
    FeatureSupport {
        dynamic_rendering: true,
        synchronization2: true,
        timeline_semaphore: true,
        maintenance4: true,
        storage_image_extended_formats: true,
        swapchain_maintenance1: true,
    }
}

fn graphics_compute_family() -> QueueFamilyReport {
    QueueFamilyReport {
        flags: vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE | vk::QueueFlags::TRANSFER,
        queue_count: 1,
        timestamp_valid_bits: 64,
    }
}

fn candidate(name: &str, device_type: vk::PhysicalDeviceType, extensions: &[&CStr]) -> DeviceCandidate {
    DeviceCandidate {
        name: name.to_owned(),
        device_type,
        api_version: vk::API_VERSION_1_3,
        extensions: extensions
            .iter()
            .map(|name| name.to_string_lossy().into_owned())
            .collect(),
        queue_families: vec![graphics_compute_family()],
        features: full_features(),
    }
}

fn requirements(required: &[&'static CStr], optional: &[&'static CStr]) -> ExtensionRequirements {
    ExtensionRequirements {
        required: required.to_vec(),
        optional: optional.to_vec(),
    }
}

#[test]
fn missing_required_extension_on_the_only_device_is_reported_by_name() {
    let devices = [candidate(
        "Единственный GPU",
        vk::PhysicalDeviceType::INTEGRATED_GPU,
        &[khr::swapchain::NAME],
    )];
    let error = select_device(&devices, &requirements(&[FAKE_MISSING_EXTENSION], &[]), &[]).unwrap_err();
    assert!(
        matches!(&error, GpuInitError::MissingDeviceExtension { name } if name == "VK_FAKE_never_exists"),
        "{error:?}"
    );
}

#[test]
fn differing_rejections_are_listed_per_device() {
    let mut old_device = candidate("Старый", vk::PhysicalDeviceType::DISCRETE_GPU, &[khr::swapchain::NAME]);
    old_device.api_version = vk::API_VERSION_1_2;
    let mut no_queue = candidate(
        "Без очереди",
        vk::PhysicalDeviceType::INTEGRATED_GPU,
        &[khr::swapchain::NAME],
    );
    no_queue.queue_families = vec![QueueFamilyReport {
        flags: vk::QueueFlags::TRANSFER,
        queue_count: 1,
        timestamp_valid_bits: 0,
    }];
    let error = select_device(
        &[old_device, no_queue],
        &requirements(&[khr::swapchain::NAME], &[]),
        &[],
    )
    .unwrap_err();
    let GpuInitError::NoSuitableDevice { rejected } = error else {
        panic!("ждали NoSuitableDevice")
    };
    assert_eq!(rejected.len(), 2);
    assert_eq!(rejected[0].device_name, "Старый");
    assert_eq!(
        rejected[0].reason,
        DeviceRejectionReason::ApiVersionTooLow {
            found: "1.2".to_owned()
        }
    );
    assert_eq!(rejected[1].reason, DeviceRejectionReason::NoGraphicsComputeQueue);
}

#[test]
fn no_devices_at_all_gives_empty_rejection_list() {
    let error = select_device(&[], &requirements(&[], &[]), &[]).unwrap_err();
    assert!(
        matches!(error, GpuInitError::NoSuitableDevice { ref rejected } if rejected.is_empty()),
        "{error:?}"
    );
}

#[test]
fn missing_mandatory_feature_is_reported_by_name() {
    let mut device = candidate("GPU", vk::PhysicalDeviceType::DISCRETE_GPU, &[]);
    device.features.synchronization2 = false;
    let error = select_device(&[device], &requirements(&[], &[]), &[]).unwrap_err();
    assert!(
        matches!(&error, GpuInitError::MissingFeature { name } if name == "synchronization2"),
        "{error:?}"
    );
}

#[test]
fn discrete_gpu_beats_integrated_and_cpu_regardless_of_order() {
    let devices = [
        candidate("llvmpipe", vk::PhysicalDeviceType::CPU, &[]),
        candidate("iGPU", vk::PhysicalDeviceType::INTEGRATED_GPU, &[]),
        candidate("dGPU", vk::PhysicalDeviceType::DISCRETE_GPU, &[]),
    ];
    let selection = select_device(&devices, &requirements(&[], &[]), &[]).unwrap();
    assert_eq!(selection.device_index, 2);
}

#[test]
fn rejected_better_device_does_not_hide_a_suitable_worse_one() {
    let devices = [
        candidate("dGPU без расширения", vk::PhysicalDeviceType::DISCRETE_GPU, &[]),
        candidate("iGPU", vk::PhysicalDeviceType::INTEGRATED_GPU, &[khr::swapchain::NAME]),
    ];
    let selection = select_device(&devices, &requirements(&[khr::swapchain::NAME], &[]), &[]).unwrap();
    assert_eq!(selection.device_index, 1);
}

#[test]
fn optional_extension_is_enabled_only_when_present() {
    let with = candidate(
        "A",
        vk::PhysicalDeviceType::DISCRETE_GPU,
        &[khr::external_fence_fd::NAME],
    );
    let without = candidate("B", vk::PhysicalDeviceType::DISCRETE_GPU, &[]);
    let wanted = requirements(&[], &[khr::external_fence_fd::NAME]);
    let enabled_with = select_device(&[with], &wanted, &[]).unwrap().enabled_extensions;
    let enabled_without = select_device(&[without], &wanted, &[]).unwrap().enabled_extensions;
    assert_eq!(enabled_with, vec![khr::external_fence_fd::NAME]);
    assert!(enabled_without.is_empty());
}

#[test]
fn swapchain_maintenance1_needs_feature_and_instance_extension() {
    let device = candidate(
        "Phone",
        vk::PhysicalDeviceType::INTEGRATED_GPU,
        &[ext::swapchain_maintenance1::NAME],
    );
    let wanted = requirements(&[], &[ext::swapchain_maintenance1::NAME]);
    // Нет instance-расширения → не включаем.
    assert!(
        select_device(std::slice::from_ref(&device), &wanted, &[])
            .unwrap()
            .enabled_extensions
            .is_empty()
    );
    // Есть всё → включаем.
    let enabled = select_device(
        std::slice::from_ref(&device),
        &wanted,
        &[ext::surface_maintenance1::NAME],
    )
    .unwrap();
    assert_eq!(enabled.enabled_extensions, vec![ext::swapchain_maintenance1::NAME]);
    // Нет фичи → не включаем.
    let mut featureless = device;
    featureless.features.swapchain_maintenance1 = false;
    let enabled = select_device(&[featureless], &wanted, &[ext::surface_maintenance1::NAME]).unwrap();
    assert!(enabled.enabled_extensions.is_empty());
}

#[test]
fn queue_family_with_graphics_and_compute_is_chosen_not_the_first() {
    let mut device = candidate("GPU", vk::PhysicalDeviceType::DISCRETE_GPU, &[]);
    device.queue_families = vec![
        QueueFamilyReport {
            flags: vk::QueueFlags::COMPUTE,
            queue_count: 4,
            timestamp_valid_bits: 64,
        },
        QueueFamilyReport {
            flags: vk::QueueFlags::GRAPHICS,
            queue_count: 1,
            timestamp_valid_bits: 64,
        },
        graphics_compute_family(),
    ];
    assert_eq!(
        select_device(&[device], &requirements(&[], &[]), &[])
            .unwrap()
            .queue_family_index,
        2
    );
}

#[test]
fn profiles_declare_contract_extensions() {
    let android_device = DeviceProfile::AndroidDisplay.device_extensions();
    for name in [
        khr::swapchain::NAME,
        ash::android::external_memory_android_hardware_buffer::NAME,
        vk::EXT_QUEUE_FAMILY_FOREIGN_NAME,
        khr::external_semaphore_fd::NAME,
        khr::external_fence_fd::NAME,
        google::display_timing::NAME,
    ] {
        assert!(android_device.required.contains(&name), "{name:?}");
    }
    assert!(android_device.optional.contains(&ext::swapchain_maintenance1::NAME));
    let android_instance = DeviceProfile::AndroidDisplay.instance_extensions();
    assert!(android_instance.required.contains(&khr::android_surface::NAME));
    assert!(DeviceProfile::Headless.instance_extensions().required.is_empty());
    assert_eq!(
        DeviceProfile::Headless.device_extensions().required,
        vec![khr::external_semaphore_fd::NAME]
    );
    assert!(
        DeviceProfile::HeadlessPresentation
            .device_extensions()
            .required
            .contains(&khr::swapchain::NAME)
    );
    assert!(
        DeviceProfile::HeadlessPresentation
            .instance_extensions()
            .required
            .contains(&ext::headless_surface::NAME)
    );
}
