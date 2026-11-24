//! Шов «откуда сценарию стенда взять GPU»: контекст отдельный от потока рендера и карта теней устройства.
//!
//! Сценарий не знает ни `VulkanDisplayBackend`, ни потока рендера. На телефоне контекст даёт долгоживущий контекст
//! разовых диагностик (`vulkan_diagnostics`): создавать и уничтожать instance на каждый запуск нельзя (факт
//! сессии 03: уничтожение instance драйвером Adreno иногда убивает поток). На ПК в сценарии контекста нет
//! (`UnavailableSaturationGpu`), а тесты подставляют свой.

use std::sync::Arc;

use camera_contract::lens_shading::LensShadingCalibrations;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::saturation::SaturationConfig;

use super::gpu_executor::GpuWorkloadExecutor;
use super::inputs::ProductInputs;
use super::runner::WorkloadExecutor;
use super::screen_scene::SurfaceSizePx;
use crate::gpu_services::ValidationState;

/// Всё, что исполнителю нужно знать о прогоне.
pub struct ExecutorSetup {
    pub inputs: ProductInputs,
    pub bench: SaturationConfig,
    pub surface: SurfaceSizePx,
}

/// Источник исполнителя работ стенда: настоящий (на GPU-контексте) или подставной (тесты сценария).
pub trait SaturationGpu: Send + Sync {
    /// Исполнитель, привязанный к GPU-контексту стенда. Вызывается в потоке прогона. Неудача не запоминается:
    /// следующий вызов пробует снова.
    fn executor(&self, setup: ExecutorSetup) -> Result<Box<dyn WorkloadExecutor>, String>;

    /// Включён ли validation layer: в debug цифры завышены, это пишется в результат.
    fn validation(&self) -> ValidationState;
}

/// GPU недоступен (сборка без стенда или инициализация провалена): сценарий падает с причиной.
#[derive(Debug, Clone)]
pub struct UnavailableSaturationGpu {
    reason: String,
}

impl UnavailableSaturationGpu {
    pub fn new(reason: impl Into<String>) -> Self {
        Self { reason: reason.into() }
    }
}

impl SaturationGpu for UnavailableSaturationGpu {
    fn executor(&self, _setup: ExecutorSetup) -> Result<Box<dyn WorkloadExecutor>, String> {
        Err(self.reason.clone())
    }

    fn validation(&self) -> ValidationState {
        ValidationState::Disabled
    }
}

/// Настоящий исполнитель на уже открытом GPU-контексте (на телефоне — контекст диагностик, в тестах — контекст теста).
pub fn executor_over_context(context: GpuContext, setup: ExecutorSetup) -> Box<dyn WorkloadExecutor> {
    Box::new(GpuWorkloadExecutor::new(
        context,
        setup.inputs,
        setup.bench,
        setup.surface,
    ))
}

/// Сервисы стенда в `GpuServices`.
#[derive(Clone)]
pub struct SaturationServices {
    pub gpu: Arc<dyn SaturationGpu>,
    /// Карта теней объектива устройства: та же, что получил бэкенд камеры.
    pub lens_shading: Arc<LensShadingCalibrations>,
}

impl SaturationServices {
    pub fn new(gpu: Arc<dyn SaturationGpu>, lens_shading: LensShadingCalibrations) -> Self {
        Self {
            gpu,
            lens_shading: Arc::new(lens_shading),
        }
    }

    /// Сервисы без GPU: стенд недоступен, карты теней нет.
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self::new(
            Arc::new(UnavailableSaturationGpu::new(reason)),
            LensShadingCalibrations::none(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera_screen_config::CameraScreenConfig;
    use crate::saturation_bench::inputs::{InputSources, gather_inputs};

    fn test_setup() -> ExecutorSetup {
        let render = render_loop::config::RenderLoopConfig::default();
        let screen = CameraScreenConfig::default();
        let lens_shading = LensShadingCalibrations::none();
        ExecutorSetup {
            inputs: gather_inputs(&InputSources {
                engine_status: None,
                lens_shading: &lens_shading,
                render_config: &render,
                look: None,
                screen: &screen,
            }),
            bench: SaturationConfig::default(),
            surface: SurfaceSizePx::ONEPLUS_11,
        }
    }

    #[test]
    fn unavailable_services_report_the_reason_instead_of_a_context() {
        let services = SaturationServices::unavailable("нет Vulkan на ПК");
        assert_eq!(
            services.gpu.executor(test_setup()).err().as_deref(),
            Some("нет Vulkan на ПК"),
            "причина отказа доходит до сценария"
        );
        assert_eq!(services.gpu.validation(), ValidationState::Disabled);
        assert_eq!(*services.lens_shading, LensShadingCalibrations::none());
    }
}
