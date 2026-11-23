//! Тестовые швы чтения результатов цветовых приборов (фича `test-support`): счётчики и картинка прибора последней
//! записи. Вынесено из `session.rs` (предел размера модуля); на горячем пути не используется.

use super::{AttachedDisplay, Painter};

impl AttachedDisplay {
    /// Счётчики приборов последней записи (тестовый шов). `Ok(None)` — камеры нет или приборов нет.
    #[cfg(feature = "test-support")]
    pub(in crate::vulkan_backend) fn read_scope_counters(
        &self,
    ) -> Result<Option<scope_model::ScopeCounters>, isp_vulkan::scopes::ScopeError> {
        match self.chain.as_ref().map(|chain| &chain.painter) {
            Some(Painter::Camera { pipelines, .. }) => pipelines.read_scope_counters(),
            _ => Ok(None),
        }
    }

    /// Картинка прибора последней записи (тестовый шов).
    #[cfg(feature = "test-support")]
    pub(in crate::vulkan_backend) fn read_scope_picture(
        &self,
    ) -> Result<Option<isp_vulkan::scopes::PictureReadback>, isp_vulkan::scopes::ScopeError> {
        match self.chain.as_ref().map(|chain| &chain.painter) {
            Some(Painter::Camera { pipelines, .. }) => pipelines.read_scope_picture(),
            _ => Ok(None),
        }
    }
}
