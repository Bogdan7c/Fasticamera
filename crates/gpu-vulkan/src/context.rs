//! `GpuContext` — разделяемый владелец Vulkan: entry, instance, messenger, device, очередь, загрузчики.
//!
//! Инварианты:
//! * контекст — `Clone` (внутри `Arc`); каждая обёртка `Owned*` держит клон, поэтому устройство
//!   живёт дольше любого своего объекта;
//! * очередь одна (graphics+compute), любая работа с ней идёт под внутренним замком
//!   (`vkQueueSubmit*`/`vkQueuePresentKHR` требуют внешней синхронизации);
//! * порядок уничтожения в `Drop`: ожидание простоя → device → messenger → instance → (entry);
//! * расширения, не включённые профилем, недоступны: загрузчик возвращает типизированную ошибку,
//!   а не `None`, который можно забыть проверить.

mod loaders;
mod memory;
mod one_shot;
mod owners;
#[cfg(test)]
pub(crate) mod test_support;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use ash::{Device, Entry, Instance, vk};

pub use self::loaders::ExtensionLoaders;
pub(crate) use self::memory::MemoryLocation;
use self::owners::{DeviceOwner, InstanceOwner};
use crate::device::probe::{DeviceDetails, probe_devices};
use crate::device::{DeviceProfile, DeviceSelection, select_device};
use crate::error::{GpuError, GpuInitError, init_step, run_step};
use crate::instance::{SyncValidationBackend, create_instance, plan_instance, query_instance_support};
use crate::live_objects::{LiveObjectCounters, LiveObjectSnapshot};
use crate::validation::{DebugMessenger, ValidationHook, ValidationMode, ValidationReport};

/// Настройки создания контекста.
#[derive(Debug, Clone)]
pub struct GpuContextConfig {
    /// Профиль: какие расширения обязательны.
    pub profile: DeviceProfile,
    /// Режим validation.
    pub validation: ValidationMode,
    /// Имя приложения для `VkApplicationInfo` (видно в отладчиках и слое validation).
    pub app_label: String,
    /// Сколько ждать завершения разовой работы на GPU (`one_shot`), прежде чем вернуть
    /// `GpuError::Timeout`. Бесконечных ожиданий в crate нет.
    pub submit_wait_timeout: Duration,
}

impl GpuContextConfig {
    /// Таймаут ожидания GPU по умолчанию: разовые задачи (readback кадра) занимают миллисекунды,
    /// 5 секунд — запас на медленный драйвер/CI, но не вечное зависание.
    pub const DEFAULT_SUBMIT_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

    /// Конфигурация с разумными значениями по умолчанию для остальных полей.
    pub fn new(profile: DeviceProfile, validation: ValidationMode) -> Self {
        Self {
            profile,
            validation,
            app_label: "fasticamera".to_owned(),
            submit_wait_timeout: Self::DEFAULT_SUBMIT_WAIT_TIMEOUT,
        }
    }
}

/// Версия Vulkan API устройства.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiVersion {
    /// Старшая.
    pub major: u32,
    /// Младшая.
    pub minor: u32,
    /// Патч.
    pub patch: u32,
}

/// Поддержка меток времени GPU на выбранной очереди.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimestampSupport {
    /// Число значимых бит метки.
    pub valid_bits: u32,
    /// Наносекунд на тик.
    pub period_ns: f32,
}

/// Необязательные 16-битные возможности шейдеров (сессия 07). Включаются, только если устройство их
/// поддерживает; ни одна не обязательна — ISP выбирает вариант шейдера по этому отчёту.
///
/// На телефоне (Adreno 740) есть обе; `uniformAndStorageBuffer16BitAccess` и `storagePushConstant16`
/// там НЕТ, поэтому они здесь не запрашиваются вовсе.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShaderSixteenBitSupport {
    /// `shaderFloat16` (Vulkan 1.2) включена: арифметика `float16_t` в шейдерах.
    pub float16_arithmetic: bool,
    /// `storageBuffer16BitAccess` (Vulkan 1.1) включена: 16-битные типы в storage-буферах.
    pub storage_buffer_16bit_access: bool,
}

/// Поведение 32-битной арифметики шейдеров по `VkPhysicalDeviceFloatControlsProperties` (Vulkan 1.2, ядро).
///
/// Это ФАКТЫ об устройстве для диагностики, не включение режимов: контекст не выставляет шейдерам
/// `RoundingModeRTE`/`DenormPreserve`/`DenormFlushToZero`. Спецификация Vulkan не говорит, какое округление
/// и какую обработку денормализованных чисел драйвер применяет по умолчанию, поэтому код, которому нужна
/// побитовая воспроизводимость (цветовые приборы, 14h), опирается на предположение «по умолчанию —
/// ближайшее чётное, денормалей на входе нет» и показывает эти факты рядом с отчётом.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Float32Controls {
    /// `shaderRoundingModeRTEFloat32`: шейдер МОЖЕТ потребовать округление к ближайшему чётному для f32.
    pub rounding_mode_rte: bool,
    /// `shaderDenormPreserveFloat32`: шейдер может потребовать сохранять денормализованные f32.
    pub denorm_preserve: bool,
    /// `shaderDenormFlushToZeroFloat32`: шейдер может потребовать сброс денормализованных f32 в ноль.
    pub denorm_flush_to_zero: bool,
}

/// Отчёт о возможностях созданного контекста (для диагностики и smoke-сценариев).
#[derive(Debug, Clone)]
pub struct GpuCapabilities {
    /// Имя устройства.
    pub device_name: String,
    /// Имя драйвера (например, `radv`).
    pub driver_name: String,
    /// Подробности драйвера.
    pub driver_info: String,
    /// Идентификатор драйвера (текстом, `Debug` от `vk::DriverId`).
    pub driver_id: String,
    /// Версия API устройства.
    pub api_version: ApiVersion,
    /// Включённые расширения instance.
    pub enabled_instance_extensions: Vec<String>,
    /// Включённые расширения устройства.
    pub enabled_device_extensions: Vec<String>,
    /// Включена ли `shaderStorageImageExtendedFormats` (нужна для `r16ui`).
    pub storage_image_extended_formats: bool,
    /// Метки времени GPU; `None` — очередь их не поддерживает.
    pub timestamps: Option<TimestampSupport>,
    /// Какие необязательные 16-битные возможности шейдеров включены.
    pub shader_16bit: ShaderSixteenBitSupport,
    /// Включена ли `hostQueryReset` (Vulkan 1.2): метки времени сбрасываются с CPU (`GpuTimestamps`).
    pub host_query_reset: bool,
    /// Возможности управления округлением и денормалями f32 в шейдерах (факт об устройстве).
    pub float32_controls: Float32Controls,
    /// Размер subgroup.
    pub subgroup_size: u32,
    /// Доступна ли арифметика subgroup (`GL_KHR_shader_subgroup_arithmetic`) в compute-шейдерах
    /// (`subgroupSupportedOperations` ∋ ARITHMETIC и `subgroupSupportedStages` ∋ COMPUTE). Без неё
    /// SPIR-V с такими операциями создавать нельзя — потребитель выбирает запасной вариант шейдера.
    pub subgroup_arithmetic_in_compute: bool,
    /// Доступны ли баллот и широковещание subgroup (`GL_KHR_shader_subgroup_ballot`) в compute-шейдерах
    /// (`subgroupSupportedOperations` ∋ BALLOT и `subgroupSupportedStages` ∋ COMPUTE). Без них SPIR-V с такими
    /// операциями создавать нельзя — потребитель выбирает запасной вариант шейдера.
    pub subgroup_ballot_in_compute: bool,
    /// Семейство очередей.
    pub queue_family_index: u32,
    /// Режим validation, с которым создан контекст.
    pub validation: ValidationMode,
    /// Как реально включена sync-валидация.
    pub sync_validation: SyncValidationBackend,
}

/// Разделяемый владелец Vulkan-объектов. Дёшево клонируется.
#[derive(Clone)]
pub struct GpuContext {
    inner: Arc<ContextInner>,
}

impl std::fmt::Debug for GpuContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "GpuContext({})", self.inner.capabilities.device_name)
    }
}

/// Внутренности. Порядок полей = порядок освобождения после `Drop` самих владельцев:
/// сначала device, затем instance (внутри него entry).
struct ContextInner {
    device: DeviceOwner,
    instance: InstanceOwner,
    loaders: ExtensionLoaders,
    queue: Mutex<vk::Queue>,
    queue_family_index: u32,
    capabilities: GpuCapabilities,
    details: DeviceDetails,
    physical_device: vk::PhysicalDevice,
    validation_hook: ValidationHook,
    live_objects: Arc<LiveObjectCounters>,
    submit_wait_timeout: Duration,
}

impl GpuContext {
    /// Создаёт контекст: Vulkan → instance → (messenger) → выбор устройства → device → очередь.
    pub fn new(config: &GpuContextConfig) -> Result<Self, GpuInitError> {
        // SAFETY: загрузка системной библиотеки Vulkan; до успеха ничего не разыменовывается.
        let entry = unsafe { Entry::load() }.map_err(|error| GpuInitError::VulkanUnavailable {
            reason: error.to_string(),
        })?;

        let instance_requirements = config.profile.instance_extensions();
        let (available_extensions, layer_present) = query_instance_support(&entry)?;
        let plan = plan_instance(
            &available_extensions,
            layer_present,
            &instance_requirements,
            &config.validation,
        )?;

        let validation_hook = ValidationHook::new(config.validation.known_deviations());
        let instance = create_instance(&entry, &plan, &config.validation, &validation_hook, &config.app_label)?;
        // Шаги инициализации — на уровне debug: по ним видно, на каком вызове драйвера встало создание.
        tracing::debug!(target: "fasticamera::gpu::init", "instance создан");
        // С этого момента instance принадлежит владельцу: при любой ошибка ниже он уничтожится сам.
        let messenger_result = if config.validation.is_enabled() {
            DebugMessenger::create(&entry, &instance, &validation_hook).map(Some)
        } else {
            Ok(None)
        };
        let mut instance_owner = InstanceOwner::new(entry, instance);
        instance_owner.set_messenger(messenger_result?);
        tracing::debug!(target: "fasticamera::gpu::init", "messenger validation подключён");

        let probed = probe_devices(instance_owner.instance())?;
        tracing::debug!(target: "fasticamera::gpu::init", devices = probed.len(), "физические устройства опрошены");
        let candidates: Vec<_> = probed.iter().map(|device| device.candidate.clone()).collect();
        let selection = select_device(&candidates, &config.profile.device_extensions(), &plan.extensions)?;
        let chosen = &probed[selection.device_index];
        tracing::debug!(target: "fasticamera::gpu::init", device_index = selection.device_index, "устройство выбрано, создаём VkDevice");

        let device = Self::create_device(
            instance_owner.instance(),
            chosen.physical_device,
            &selection,
            &chosen.candidate,
        )?;
        tracing::debug!(target: "fasticamera::gpu::init", "VkDevice создан");
        let device_owner = DeviceOwner::new(device);
        let loaders = ExtensionLoaders::new(
            instance_owner.entry(),
            instance_owner.instance(),
            device_owner.handle(),
            &plan.extensions,
            &selection.enabled_extensions,
        );
        // SAFETY: семейство очередей и индекс 0 были запрошены в create_device.
        let queue = unsafe { device_owner.handle().get_device_queue(selection.queue_family_index, 0) };

        let capabilities = Self::build_capabilities(config, &plan.extensions, plan.sync_validation, &selection, chosen);
        tracing::info!(
            target: "fasticamera::gpu",
            device = %capabilities.device_name,
            driver = %capabilities.driver_name,
            api = ?capabilities.api_version,
            validation = ?config.validation,
            sync_validation = ?capabilities.sync_validation,
            "GpuContext создан"
        );

        Ok(Self {
            inner: Arc::new(ContextInner {
                device: device_owner,
                instance: instance_owner,
                loaders,
                queue: Mutex::new(queue),
                queue_family_index: selection.queue_family_index,
                capabilities,
                details: chosen.details.clone(),
                physical_device: chosen.physical_device,
                validation_hook,
                live_objects: Arc::new(LiveObjectCounters::default()),
                submit_wait_timeout: config.submit_wait_timeout,
            }),
        })
    }

    /// Создаёт логическое устройство с одной очередью и обязательными фичами Vulkan 1.3.
    fn create_device(
        instance: &Instance,
        physical_device: vk::PhysicalDevice,
        selection: &DeviceSelection,
        candidate: &crate::device::DeviceCandidate,
    ) -> Result<Device, GpuInitError> {
        let queue_priorities = [1.0_f32];
        let queue_infos = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(selection.queue_family_index)
            .queue_priorities(&queue_priorities)];
        let extension_pointers: Vec<*const std::ffi::c_char> =
            selection.enabled_extensions.iter().map(|name| name.as_ptr()).collect();

        let enable_maintenance1 = selection
            .enabled_extensions
            .contains(&ash::ext::swapchain_maintenance1::NAME);
        // 16-битные возможности необязательны (сессия 07): включаем ровно то, что устройство поддерживает.
        let mut features_11 = vk::PhysicalDeviceVulkan11Features::default()
            .storage_buffer16_bit_access(candidate.features.storage_buffer_16bit_access);
        let mut features_12 = vk::PhysicalDeviceVulkan12Features::default()
            .timeline_semaphore(true)
            .shader_float16(candidate.features.shader_float16)
            // Сброс меток времени с CPU (`GpuTimestamps`): включаем, только если устройство умеет.
            .host_query_reset(candidate.features.host_query_reset);
        let mut features_13 = vk::PhysicalDeviceVulkan13Features::default()
            .dynamic_rendering(true)
            .synchronization2(true)
            // Нужна для `LocalSizeId` в compute-шейдерах, см. `FeatureSupport::maintenance4`.
            .maintenance4(true);
        let mut maintenance1_features =
            vk::PhysicalDeviceSwapchainMaintenance1FeaturesEXT::default().swapchain_maintenance1(true);
        // Расширенные форматы storage image включаем, только если устройство их поддерживает.
        let base_features = vk::PhysicalDeviceFeatures::default()
            .shader_storage_image_extended_formats(candidate.features.storage_image_extended_formats);
        let mut features2 = vk::PhysicalDeviceFeatures2::default()
            .features(base_features)
            .push_next(&mut features_11)
            .push_next(&mut features_12)
            .push_next(&mut features_13);
        if enable_maintenance1 {
            features2 = features2.push_next(&mut maintenance1_features);
        }
        let create_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_infos)
            .enabled_extension_names(&extension_pointers)
            .push_next(&mut features2);

        // SAFETY: physical_device получен из этого instance; create_info и цепочка pNext живут до конца вызова.
        init_step(
            unsafe { instance.create_device(physical_device, &create_info, None) },
            "vkCreateDevice",
        )
    }

    /// Собирает публичный отчёт о возможностях.
    fn build_capabilities(
        config: &GpuContextConfig,
        instance_extensions: &[&std::ffi::CStr],
        sync_validation: SyncValidationBackend,
        selection: &DeviceSelection,
        chosen: &crate::device::probe::ProbedDevice,
    ) -> GpuCapabilities {
        let family = chosen.candidate.queue_families[selection.queue_family_index as usize];
        let version = chosen.candidate.api_version;
        GpuCapabilities {
            device_name: chosen.candidate.name.clone(),
            driver_name: chosen.details.driver_name.clone(),
            driver_info: chosen.details.driver_info.clone(),
            driver_id: format!("{:?}", chosen.details.driver_id),
            api_version: ApiVersion {
                major: vk::api_version_major(version),
                minor: vk::api_version_minor(version),
                patch: vk::api_version_patch(version),
            },
            enabled_instance_extensions: instance_extensions
                .iter()
                .map(|name| name.to_string_lossy().into_owned())
                .collect(),
            enabled_device_extensions: selection
                .enabled_extensions
                .iter()
                .map(|name| name.to_string_lossy().into_owned())
                .collect(),
            storage_image_extended_formats: chosen.candidate.features.storage_image_extended_formats,
            timestamps: (family.timestamp_valid_bits > 0).then_some(TimestampSupport {
                valid_bits: family.timestamp_valid_bits,
                period_ns: chosen.details.timestamp_period_ns,
            }),
            shader_16bit: ShaderSixteenBitSupport {
                float16_arithmetic: chosen.candidate.features.shader_float16,
                storage_buffer_16bit_access: chosen.candidate.features.storage_buffer_16bit_access,
            },
            host_query_reset: chosen.candidate.features.host_query_reset,
            float32_controls: chosen.details.float32_controls,
            subgroup_size: chosen.details.subgroup_size,
            subgroup_arithmetic_in_compute: chosen.details.subgroup_arithmetic_in_compute,
            subgroup_ballot_in_compute: chosen.details.subgroup_ballot_in_compute,
            queue_family_index: selection.queue_family_index,
            validation: config.validation.clone(),
            sync_validation,
        }
    }

    /// Возможности устройства и включённые расширения.
    pub fn capabilities(&self) -> &GpuCapabilities {
        &self.inner.capabilities
    }

    /// Накопленные сообщения validation (всё время жизни контекста).
    pub fn validation_report(&self) -> ValidationReport {
        self.inner.validation_hook.report()
    }

    /// Снимок числа живых объектов Vulkan по видам.
    pub fn live_objects(&self) -> LiveObjectSnapshot {
        self.inner.live_objects.snapshot()
    }

    /// Ждёт, пока GPU закончит всю отправленную работу (`vkDeviceWaitIdle` под замком очереди).
    pub fn wait_idle(&self) -> Result<(), GpuError> {
        let _queue_guard = self.lock_queue();
        // SAFETY: устройство валидно; очередь заблокирована, внешняя синхронизация соблюдена.
        run_step(unsafe { self.device().device_wait_idle() }, "vkDeviceWaitIdle")
    }

    /// Блокирует очередь. Отравленный замок (паника в другом потоке) не фатален: `vk::Queue` — просто handle.
    fn lock_queue(&self) -> std::sync::MutexGuard<'_, vk::Queue> {
        self.inner.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    // ---- доступ для модулей crate (sync, swapchain, android, offscreen, test_pattern) ----

    /// Логическое устройство.
    pub(crate) fn device(&self) -> &Device {
        self.inner.device.handle()
    }

    /// Instance.
    pub(crate) fn instance(&self) -> &Instance {
        self.inner.instance.instance()
    }

    /// Физическое устройство.
    pub(crate) fn physical_device(&self) -> vk::PhysicalDevice {
        self.inner.physical_device
    }

    /// Семейство единственной очереди.
    pub(crate) fn queue_family_index(&self) -> u32 {
        self.inner.queue_family_index
    }

    /// Счётчики живых объектов (для `Owned*`).
    pub(crate) fn live_object_counters(&self) -> &Arc<LiveObjectCounters> {
        &self.inner.live_objects
    }

    /// Максимальный размер 2D-картинки устройства.
    pub(crate) fn max_image_dimension_2d(&self) -> u32 {
        self.inner.details.max_image_dimension_2d
    }

    /// Возможности формата при `OPTIMAL`-тайлинге (выбор формата storage image).
    pub(crate) fn optimal_tiling_features(&self, format: vk::Format) -> vk::FormatFeatureFlags {
        // SAFETY: физическое устройство получено из этого instance; вызов только читает свойства.
        let properties = unsafe {
            self.instance()
                .get_physical_device_format_properties(self.physical_device(), format)
        };
        properties.optimal_tiling_features
    }

    /// Максимальный размер push-констант устройства, байт.
    pub(crate) fn max_push_constants_size(&self) -> u32 {
        self.inner.details.max_push_constants_size
    }

    /// Максимальное число рабочих групп одного dispatch по осям X, Y, Z.
    pub(crate) fn max_compute_work_group_count(&self) -> [u32; 3] {
        self.inner.details.max_compute_work_group_count
    }

    /// Лимиты размера рабочей группы устройства (`maxComputeWorkGroupInvocations`, `maxComputeWorkGroupSize`).
    pub(crate) fn compute_work_group_limits(&self) -> crate::compute::WorkgroupSizeLimits {
        crate::compute::WorkgroupSizeLimits {
            max_invocations: self.inner.details.max_compute_work_group_invocations,
            max_size: self.inner.details.max_compute_work_group_size,
        }
    }

    /// Лимиты диапазонов буферов в дескрипторах (`maxStorageBufferRange`, `maxUniformBufferRange`).
    pub(crate) fn descriptor_range_limits(&self) -> crate::compute::DescriptorRangeLimits {
        crate::compute::DescriptorRangeLimits {
            max_storage_buffer_range: u64::from(self.inner.details.max_storage_buffer_range),
            max_uniform_buffer_range: u64::from(self.inner.details.max_uniform_buffer_range),
        }
    }

    /// Включена ли `hostQueryReset` (сброс запросов с CPU, `vkResetQueryPool`).
    pub(crate) fn host_query_reset_enabled(&self) -> bool {
        self.inner.capabilities.host_query_reset
    }

    /// Таймаут ожидания разовых задач GPU из конфигурации.
    pub(crate) fn submit_wait_timeout(&self) -> Duration {
        self.inner.submit_wait_timeout
    }

    /// Загрузчики расширений (включённых профилем).
    pub(crate) fn loaders(&self) -> &ExtensionLoaders {
        &self.inner.loaders
    }

    /// Отправляет работу на единственную очередь (`vkQueueSubmit2`) под внутренним замком.
    ///
    /// Ошибки не сливаются: любой код Vulkan возвращается как `GpuError::Vulkan` с именем шага.
    pub(crate) fn submit(&self, submits: &[vk::SubmitInfo2<'_>], fence: vk::Fence) -> Result<(), GpuError> {
        let queue = self.lock_queue();
        // SAFETY: очередь принадлежит этому устройству и заблокирована (внешняя синхронизация);
        // submits ссылаются на живые структуры вызывающего.
        run_step(
            unsafe { self.device().queue_submit2(*queue, submits, fence) },
            "vkQueueSubmit2",
        )
    }

    /// Выполняет `operation` с очередью под замком (для `vkQueuePresentKHR` и подобных вызовов).
    pub(crate) fn with_queue<T>(&self, operation: impl FnOnce(vk::Queue) -> T) -> T {
        let queue = self.lock_queue();
        operation(*queue)
    }
}

#[cfg(test)]
mod tests;
