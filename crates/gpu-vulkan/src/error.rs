//! Ошибки `gpu-vulkan`.
//!
//! Два разных типа, потому что вызывающему важно различие:
//! * [`GpuInitError`] — контекст не удалось создать (нет Vulkan, нет расширения, нет подходящего GPU);
//!   это состояние «система не годится», его показывают пользователю/диагностике;
//! * [`GpuError`] — ошибка работы уже созданного контекста: код Vulkan, таймаут ожидания GPU,
//!   запрошенная возможность не включена или не поддерживается.
//!
//! Отсутствие ресурса, таймаут, «не поддерживается» и фатальный код Vulkan не сливаются в один `bool`.

use ash::vk;

/// Причина, по которой физическое устройство отвергнуто при выборе.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceRejectionReason {
    /// Устройство поддерживает Vulkan ниже требуемой версии (нужен 1.3).
    ApiVersionTooLow {
        /// Найденная версия в виде `major.minor`.
        found: String,
    },
    /// Нет обязательного расширения устройства.
    MissingDeviceExtension {
        /// Имя расширения.
        name: String,
    },
    /// Нет обязательной возможности (feature) устройства.
    MissingFeature {
        /// Имя возможности.
        name: String,
    },
    /// Нет семейства очередей с GRAPHICS и COMPUTE одновременно.
    NoGraphicsComputeQueue,
}

/// Отказ по одному физическому устройству: что за устройство и почему оно не подошло.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRejection {
    /// Имя устройства из драйвера.
    pub device_name: String,
    /// Причина отказа.
    pub reason: DeviceRejectionReason,
}

/// Ошибка создания [`crate::context::GpuContext`].
#[derive(Debug, thiserror::Error)]
pub enum GpuInitError {
    /// Загрузчик Vulkan (`libvulkan`) не найден или не открылся.
    #[error("Vulkan недоступен: {reason}")]
    VulkanUnavailable {
        /// Текст ошибки загрузки библиотеки.
        reason: String,
    },
    /// Загрузчик не поддерживает обязательное расширение instance.
    #[error("нет обязательного расширения instance {name}")]
    MissingInstanceExtension {
        /// Имя расширения.
        name: String,
    },
    /// Ни одно устройство не поддерживает обязательное расширение устройства.
    #[error("нет обязательного расширения устройства {name}")]
    MissingDeviceExtension {
        /// Имя расширения.
        name: String,
    },
    /// Ни одно устройство не поддерживает обязательную возможность.
    #[error("нет обязательной возможности устройства {name}")]
    MissingFeature {
        /// Имя возможности.
        name: String,
    },
    /// Включена validation, но слоя `VK_LAYER_KHRONOS_validation` в системе нет.
    #[error("слой VK_LAYER_KHRONOS_validation недоступен")]
    ValidationLayerUnavailable,
    /// Подходящего устройства нет; причины отказа — по каждому устройству.
    #[error("нет подходящего Vulkan-устройства: {}", format_rejections(.rejected))]
    NoSuitableDevice {
        /// Отказ по каждому найденному устройству (пусто — устройств нет вообще).
        rejected: Vec<DeviceRejection>,
    },
    /// Вызов Vulkan вернул ошибку.
    #[error("вызов Vulkan {step} завершился ошибкой {result:?}")]
    Vulkan {
        /// Какой шаг инициализации упал.
        step: &'static str,
        /// Код возврата Vulkan.
        result: vk::Result,
    },
}

/// Человекочитаемый список отказов для сообщения `NoSuitableDevice`.
fn format_rejections(rejected: &[DeviceRejection]) -> String {
    if rejected.is_empty() {
        return "физических устройств нет".to_owned();
    }
    rejected
        .iter()
        .map(|rejection| format!("{}: {:?}", rejection.device_name, rejection.reason))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Ошибка работы созданного контекста.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GpuError {
    /// Вызов Vulkan вернул ошибку.
    #[error("вызов Vulkan {step} завершился ошибкой {result:?}")]
    Vulkan {
        /// Какой шаг упал.
        step: &'static str,
        /// Код возврата Vulkan.
        result: vk::Result,
    },
    /// GPU не закончил работу за отведённое время. Объекты, которыми он мог пользоваться,
    /// намеренно не уничтожаются (см. `context::one_shot`): считайте контекст подозрительным.
    #[error("таймаут ожидания GPU на шаге {step}")]
    Timeout {
        /// Какое ожидание истекло.
        step: &'static str,
    },
    /// Нужное расширение не было включено в профиле контекста.
    #[error("расширение {name} не включено в профиле контекста")]
    ExtensionNotEnabled {
        /// Имя расширения.
        name: &'static str,
    },
    /// Запрошенное не поддерживается (например, зеркальный `preTransform` в тестовом паттерне).
    #[error("не поддерживается: {what}")]
    Unsupported {
        /// Что именно не поддерживается.
        what: String,
    },
    /// Неверный аргумент вызова (нулевой размер, превышение лимитов устройства).
    #[error("неверный аргумент: {what}")]
    InvalidArgument {
        /// Что именно неверно.
        what: String,
    },
    /// Операция вызвана в неподходящем состоянии объекта (например, `readback` до рендера).
    #[error("неверное состояние: {what}")]
    InvalidState {
        /// Что именно неверно.
        what: String,
    },
    /// Нет подходящего типа памяти для требований ресурса.
    #[error("нет подходящего типа памяти для {purpose}")]
    NoMemoryType {
        /// Для чего искали память.
        purpose: &'static str,
    },
}

/// Превращает результат ash в `GpuInitError::Vulkan` с именем шага.
pub(crate) fn init_step<T>(result: Result<T, vk::Result>, step: &'static str) -> Result<T, GpuInitError> {
    result.map_err(|code| GpuInitError::Vulkan { step, result: code })
}

/// Превращает результат ash в `GpuError::Vulkan` с именем шага.
pub(crate) fn run_step<T>(result: Result<T, vk::Result>, step: &'static str) -> Result<T, GpuError> {
    result.map_err(|code| GpuError::Vulkan { step, result: code })
}
