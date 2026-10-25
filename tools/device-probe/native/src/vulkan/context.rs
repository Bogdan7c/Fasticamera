//! Лениво создаваемый Vulkan-контекст: instance + device на Adreno, command pool и
//! compute-пайплайны. Создаётся один раз на первом кадре (кэшируется в `shared_context`).

use std::ffi::{CStr, c_char};
use std::io::Cursor;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use ash::{Device, Entry, Instance, khr, vk};

use super::vk_step;
use crate::error::ProbeError;

/// Скомпилированные `build.rs` шейдеры (SPIR-V лежит в OUT_DIR, в git не попадает).
const SUM_R16_SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sum_r16.spv"));
const SUM_WORDS_SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sum_words.spv"));

/// Размер push-констант шейдера `sum_r16` (width, height).
pub const IMAGE_PUSH_CONSTANT_BYTES: u32 = 8;
/// Размер push-констант шейдера `sum_words` (row_words, stride_words, rows).
pub const BUFFER_PUSH_CONSTANT_BYTES: u32 = 12;

/// Один compute-пайплайн с его layout'ами.
pub struct ComputePipeline {
    pub descriptor_set_layout: vk::DescriptorSetLayout,
    pub pipeline_layout: vk::PipelineLayout,
    pub pipeline: vk::Pipeline,
}

/// Возможности устройства, влияющие на поведение пробы.
pub struct DeviceCaps {
    pub device_name: String,
    pub memory: vk::PhysicalDeviceMemoryProperties,
    /// Число значимых бит timestamp в очереди (0 — timestamp не поддержан).
    pub timestamp_valid_bits: u32,
    /// Наносекунд на тик timestamp.
    pub timestamp_period_ns: f32,
    /// Включена ли фича `shaderStorageImageExtendedFormats` (нужна для `r16ui`).
    pub storage_image_extended_formats: bool,
}

/// Vulkan-устройство + кэшированные объекты. Всё освобождается в `Drop`.
pub struct VulkanContext {
    _entry: Entry,
    instance: Instance,
    pub physical_device: vk::PhysicalDevice,
    pub device: Device,
    pub queue: vk::Queue,
    pub queue_family_index: u32,
    pub command_pool: vk::CommandPool,
    pub ahb_loader: ash::android::external_memory_android_hardware_buffer::Device,
    /// `None`, если расширения `VK_KHR_external_semaphore_fd` нет.
    pub semaphore_fd_loader: Option<khr::external_semaphore_fd::Device>,
    pub caps: DeviceCaps,
    pub image_pipeline: ComputePipeline,
    pub buffer_pipeline: ComputePipeline,
    /// Сериализует работу с единственной очередью (vkQueueSubmit требует внешней синхронизации).
    queue_lock: Mutex<()>,
}

/// Читает имена расширений устройства.
fn device_extension_names(instance: &Instance, physical: vk::PhysicalDevice) -> Vec<String> {
    // SAFETY: instance и physical валидны (получены из этого же instance).
    let properties = unsafe { instance.enumerate_device_extension_properties(physical) }.unwrap_or_else(|error| {
        log::warn!("enumerate_device_extension_properties: {error:?}");
        Vec::new()
    });
    properties
        .iter()
        .filter_map(|property| property.extension_name_as_c_str().ok())
        .map(|name| name.to_string_lossy().into_owned())
        .collect()
}

/// Имена расширений устройства для отчёта `vulkanInfo` (публично, чтобы не дублировать код).
pub fn list_device_extensions(instance: &Instance, physical: vk::PhysicalDevice) -> Vec<String> {
    device_extension_names(instance, physical)
}

/// Создаёт instance с API 1.1: на нём доступны `*2`-функции и promoted-расширения
/// (external memory/semaphore/fence, ycbcr, dedicated allocation), которые нужны для AHB.
pub fn create_instance(entry: &Entry) -> Result<Instance, String> {
    let application = vk::ApplicationInfo::default()
        .application_name(c"FasticameraProbe")
        .api_version(vk::make_api_version(0, 1, 1, 0));
    let create_info = vk::InstanceCreateInfo::default().application_info(&application);
    // SAFETY: create_info ссылается на живые локальные структуры; аллокатор по умолчанию.
    vk_step(unsafe { entry.create_instance(&create_info, None) }, "vkCreateInstance")
}

/// Загружает Vulkan loader динамически (`libvulkan.so`).
pub fn load_entry() -> Result<Entry, String> {
    // SAFETY: загрузка системной библиотеки; ничего не разыменовывается до успешной загрузки.
    unsafe { Entry::load() }.map_err(|error| format!("Entry::load (libvulkan.so): {error}"))
}

/// Выбор физического устройства: первое с расширением AHB и compute-очередью.
fn pick_device(instance: &Instance) -> Result<(vk::PhysicalDevice, u32), String> {
    // SAFETY: instance валиден.
    let devices = vk_step(
        unsafe { instance.enumerate_physical_devices() },
        "vkEnumeratePhysicalDevices",
    )?;
    for physical in devices {
        let extensions = device_extension_names(instance, physical);
        let has_ahb = extensions
            .iter()
            .any(|name| name.as_str() == ash::android::external_memory_android_hardware_buffer::NAME.to_string_lossy());
        if !has_ahb {
            continue;
        }
        // SAFETY: physical получен из этого instance.
        let families = unsafe { instance.get_physical_device_queue_family_properties(physical) };
        if let Some(index) = families
            .iter()
            .position(|family| family.queue_flags.contains(vk::QueueFlags::COMPUTE))
        {
            return Ok((physical, index as u32));
        }
    }
    Err("нет физического устройства с VK_ANDROID_external_memory_android_hardware_buffer и compute-очередью".to_owned())
}

/// Создаёт логическое устройство с нужными расширениями. Возвращает также флаг фичи.
fn create_device(
    instance: &Instance,
    physical: vk::PhysicalDevice,
    queue_family_index: u32,
) -> Result<(Device, bool), String> {
    let available = device_extension_names(instance, physical);
    let has = |name: &CStr| {
        available
            .iter()
            .any(|candidate| candidate.as_str() == name.to_string_lossy())
    };

    // Эти два обязательны: AHB-расширение формально зависит от queue_family_foreign.
    let required = [
        ash::android::external_memory_android_hardware_buffer::NAME,
        vk::EXT_QUEUE_FAMILY_FOREIGN_NAME,
    ];
    if let Some(missing) = required.iter().find(|name| !has(name)) {
        return Err(format!("устройство не поддерживает {}", missing.to_string_lossy()));
    }
    let mut extension_ptrs: Vec<*const c_char> = required.iter().map(|name| name.as_ptr()).collect();
    // Опциональные: их отсутствие просто включает CPU-fallback в import_test.
    for optional in [khr::external_semaphore_fd::NAME, khr::external_fence_fd::NAME] {
        if has(optional) {
            extension_ptrs.push(optional.as_ptr());
        }
    }

    // SAFETY: physical валиден.
    let supported = unsafe { instance.get_physical_device_features(physical) };
    let extended_formats = supported.shader_storage_image_extended_formats == vk::TRUE;
    let mut features = vk::PhysicalDeviceFeatures2::default()
        .features(vk::PhysicalDeviceFeatures::default().shader_storage_image_extended_formats(extended_formats));
    let mut ycbcr = vk::PhysicalDeviceSamplerYcbcrConversionFeatures::default().sampler_ycbcr_conversion(true);
    let priorities = [1.0_f32];
    let queue_infos = [vk::DeviceQueueCreateInfo::default()
        .queue_family_index(queue_family_index)
        .queue_priorities(&priorities)];
    let create_info = vk::DeviceCreateInfo::default()
        .queue_create_infos(&queue_infos)
        .enabled_extension_names(&extension_ptrs)
        .push_next(&mut features)
        .push_next(&mut ycbcr);
    // SAFETY: все указатели в create_info живут до конца вызова.
    let device = vk_step(
        unsafe { instance.create_device(physical, &create_info, None) },
        "vkCreateDevice",
    )?;
    Ok((device, extended_formats))
}

/// Создаёт compute-пайплайн из SPIR-V с двумя дескрипторами (binding 0 и 1).
fn create_compute_pipeline(
    device: &Device,
    spirv_bytes: &[u8],
    binding0_type: vk::DescriptorType,
    push_constant_bytes: u32,
) -> Result<ComputePipeline, String> {
    let code = ash::util::read_spv(&mut Cursor::new(spirv_bytes)).map_err(|error| format!("разбор SPIR-V: {error}"))?;
    let bindings = [
        vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(binding0_type)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE),
        vk::DescriptorSetLayoutBinding::default()
            .binding(1)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE),
    ];
    let push_ranges = [vk::PushConstantRange::default()
        .stage_flags(vk::ShaderStageFlags::COMPUTE)
        .offset(0)
        .size(push_constant_bytes)];

    // SAFETY: device валиден; create_info-структуры живут до конца вызовов.
    unsafe {
        let set_layout = vk_step(
            device
                .create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings), None),
            "vkCreateDescriptorSetLayout",
        )?;
        let set_layouts = [set_layout];
        let pipeline_layout = vk_step(
            device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&set_layouts)
                    .push_constant_ranges(&push_ranges),
                None,
            ),
            "vkCreatePipelineLayout",
        )?;
        let module = vk_step(
            device.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&code), None),
            "vkCreateShaderModule",
        )?;
        let stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(module)
            .name(c"main");
        let pipeline_info = vk::ComputePipelineCreateInfo::default()
            .stage(stage)
            .layout(pipeline_layout);
        let created = device
            .create_compute_pipelines(vk::PipelineCache::null(), &[pipeline_info], None)
            .map_err(|(_, code)| format!("vkCreateComputePipelines: {code:?}"));
        // Модуль после создания пайплайна не нужен.
        device.destroy_shader_module(module, None);
        let pipeline = created?
            .into_iter()
            .next()
            .ok_or("vkCreateComputePipelines: пустой результат")?;
        Ok(ComputePipeline {
            descriptor_set_layout: set_layout,
            pipeline_layout,
            pipeline,
        })
    }
}

impl VulkanContext {
    /// Полная инициализация (instance → device → pool → pipelines).
    fn create() -> Result<Self, String> {
        let entry = load_entry()?;
        let instance = create_instance(&entry)?;
        match Self::create_on_instance(entry, instance.clone()) {
            Ok(context) => Ok(context),
            Err(error) => {
                // SAFETY: контекст не создан, значит instance не использует никто, кроме нас.
                unsafe { instance.destroy_instance(None) };
                Err(error)
            }
        }
    }

    fn create_on_instance(entry: Entry, instance: Instance) -> Result<Self, String> {
        let (physical_device, queue_family_index) = pick_device(&instance)?;
        let (device, storage_image_extended_formats) = create_device(&instance, physical_device, queue_family_index)?;

        // SAFETY: physical_device валиден, device только что создан.
        let (properties, memory, families) = unsafe {
            (
                instance.get_physical_device_properties(physical_device),
                instance.get_physical_device_memory_properties(physical_device),
                instance.get_physical_device_queue_family_properties(physical_device),
            )
        };
        let device_name = properties
            .device_name_as_c_str()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "<неизвестно>".to_owned());
        let timestamp_valid_bits = families
            .get(queue_family_index as usize)
            .map_or(0, |family| family.timestamp_valid_bits);

        let extensions = device_extension_names(&instance, physical_device);
        let semaphore_fd_loader = extensions
            .iter()
            .any(|name| name.as_str() == khr::external_semaphore_fd::NAME.to_string_lossy())
            .then(|| khr::external_semaphore_fd::Device::new(&instance, &device));

        // SAFETY: device валиден; индекс семейства проверен при выборе устройства.
        let queue = unsafe { device.get_device_queue(queue_family_index, 0) };
        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(queue_family_index)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        // SAFETY: pool_info валиден.
        let command_pool = vk_step(
            unsafe { device.create_command_pool(&pool_info, None) },
            "vkCreateCommandPool",
        )?;

        let image_pipeline = create_compute_pipeline(
            &device,
            SUM_R16_SPIRV,
            vk::DescriptorType::STORAGE_IMAGE,
            IMAGE_PUSH_CONSTANT_BYTES,
        )?;
        let buffer_pipeline = create_compute_pipeline(
            &device,
            SUM_WORDS_SPIRV,
            vk::DescriptorType::STORAGE_BUFFER,
            BUFFER_PUSH_CONSTANT_BYTES,
        )?;

        Ok(Self {
            ahb_loader: ash::android::external_memory_android_hardware_buffer::Device::new(&instance, &device),
            _entry: entry,
            instance,
            physical_device,
            device,
            queue,
            queue_family_index,
            command_pool,
            semaphore_fd_loader,
            caps: DeviceCaps {
                device_name,
                memory,
                timestamp_valid_bits,
                timestamp_period_ns: properties.limits.timestamp_period,
                storage_image_extended_formats,
            },
            image_pipeline,
            buffer_pipeline,
            queue_lock: Mutex::new(()),
        })
    }

    /// Захват очереди на время обработки кадра.
    pub fn lock_queue(&self) -> MutexGuard<'_, ()> {
        self.queue_lock.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Доступ к instance (для запросов свойств).
    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// Индекс типа памяти, разрешённый маской `allowed_bits` и имеющий все `required` флаги.
    pub fn find_memory_type(&self, allowed_bits: u32, required: vk::MemoryPropertyFlags) -> Option<u32> {
        (0..self.caps.memory.memory_type_count).find(|index| {
            let allowed = allowed_bits & (1 << index) != 0;
            let flags = self.caps.memory.memory_types[*index as usize].property_flags;
            allowed && flags.contains(required)
        })
    }
}

impl Drop for VulkanContext {
    fn drop(&mut self) {
        // SAFETY: уничтожаем объекты в обратном порядке создания; перед этим ждём простоя устройства.
        unsafe {
            if let Err(error) = self.device.device_wait_idle() {
                log::warn!("device_wait_idle при уничтожении контекста: {error:?}");
            }
            for pipeline in [&self.image_pipeline, &self.buffer_pipeline] {
                self.device.destroy_pipeline(pipeline.pipeline, None);
                self.device.destroy_pipeline_layout(pipeline.pipeline_layout, None);
                self.device
                    .destroy_descriptor_set_layout(pipeline.descriptor_set_layout, None);
            }
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

static SHARED_CONTEXT: Mutex<Option<Arc<VulkanContext>>> = Mutex::new(None);

/// Возвращает общий контекст, создавая его при первом обращении (дальше — из кэша).
/// Очередь Vulkan одна и требует внешней синхронизации: на время работы с ней
/// вызывающий берёт [`VulkanContext::lock_queue`].
pub fn shared_context() -> Result<Arc<VulkanContext>, ProbeError> {
    let mut slot = SHARED_CONTEXT.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(existing) = slot.as_ref() {
        return Ok(Arc::clone(existing));
    }
    let context = VulkanContext::create().map_err(|error| ProbeError::platform(format!("vulkan context: {error}")))?;
    let context = Arc::new(context);
    *slot = Some(Arc::clone(&context));
    Ok(context)
}
