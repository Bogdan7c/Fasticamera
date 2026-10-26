//! Владельцы нижнего уровня: логическое устройство и instance (с messenger и entry).
//!
//! Они отдельные типы, а не код в `Drop` контекста, потому что instance должен
//! уничтожаться и при ошибке создания на полпути (до того, как контекст собран).

use ash::{Device, Entry, Instance};

use crate::validation::DebugMessenger;

/// Владелец `VkDevice`: перед уничтожением ждёт простоя GPU.
pub(super) struct DeviceOwner {
    device: Device,
}

impl DeviceOwner {
    pub(super) fn new(device: Device) -> Self {
        Self { device }
    }

    pub(super) fn handle(&self) -> &Device {
        &self.device
    }
}

impl Drop for DeviceOwner {
    fn drop(&mut self) {
        // SAFETY: устройство ещё валидно; к этому моменту все Owned* уже уничтожены (они держат
        // клон контекста, так что сюда мы попадаем только после них), очередь используется только нами.
        unsafe {
            if let Err(result) = self.device.device_wait_idle() {
                tracing::error!(target: "fasticamera::gpu", ?result, "vkDeviceWaitIdle при уничтожении устройства");
            }
            self.device.destroy_device(None);
        }
    }
}

/// Владелец `VkInstance` + messenger + загрузчик. Порядок: messenger → instance → entry.
pub(super) struct InstanceOwner {
    messenger: Option<DebugMessenger>,
    instance: Instance,
    // Entry держит библиотеку Vulkan загруженной; объявлен последним и освобождается после `drop`.
    entry: Entry,
}

impl InstanceOwner {
    pub(super) fn new(entry: Entry, instance: Instance) -> Self {
        Self {
            messenger: None,
            instance,
            entry,
        }
    }

    pub(super) fn set_messenger(&mut self, messenger: Option<DebugMessenger>) {
        self.messenger = messenger;
    }

    pub(super) fn instance(&self) -> &Instance {
        &self.instance
    }

    pub(super) fn entry(&self) -> &Entry {
        &self.entry
    }
}

impl Drop for InstanceOwner {
    fn drop(&mut self) {
        if let Some(messenger) = self.messenger.take() {
            messenger.destroy();
        }
        // SAFETY: устройство, созданное из instance, уже уничтожено (его владелец объявлен в контексте раньше);
        // messenger уничтожен выше.
        unsafe { self.instance.destroy_instance(None) };
    }
}
