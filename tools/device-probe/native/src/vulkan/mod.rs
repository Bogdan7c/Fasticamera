//! Vulkan-часть пробы: контекст устройства, импорт `AHardwareBuffer`, compute-контрольная
//! сумма и sync_fd. Везде unsafe — это вызовы `ash`; у каждого блока свой `// SAFETY:`.

pub mod ahb_import;
pub mod compute;
pub mod context;
pub mod info;
pub mod resources;
pub mod sync_fd;

/// Ошибка Vulkan-шага как текст для отчёта: `"<этап>: <VkResult>"`.
/// Для отчёта важно *какой именно* этап и код отказал, поэтому строка, а не общий bool.
pub fn vk_step<T>(result: ash::prelude::VkResult<T>, step: &str) -> Result<T, String> {
    result.map_err(|code| format!("{step}: {code:?}"))
}
