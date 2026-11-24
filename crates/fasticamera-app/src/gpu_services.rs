//! Что сценарии знают о GPU: состояние потока рендера и разовые GPU-диагностики.
//!
//! Сценарии не зависят ни от `VulkanDisplayBackend`, ни от потока рендера, ни от `GpuContext`:
//! им нужны два узких контракта.
//! * [`RenderStatusSource`] — «что сейчас делает поток рендера» (снимок [`RenderStatus`] либо честное
//!   «рендер недоступен, потому что …»);
//! * [`GpuDiagnostics`] — разовые проверки на отдельном `GpuContext` (`ahb-import`, `gpu-fence`).
//!
//! На телефоне их реализуют `render_wiring` и `vulkan_diagnostics`, в тестах — fake-объекты.

use std::collections::BTreeMap;
use std::sync::Arc;

use gpu_vulkan::validation::ValidationMode;
use render_loop::config::RenderLoopConfig;
use render_loop::thread::RenderStatus;
use serde::Serialize;

use crate::saturation_bench::services::SaturationServices;
use crate::viewfinder_control::{RenderControl, RenderScenarioClaim};

/// Включён ли validation layer: в release его нет, и «0 ошибок» там ничего не доказывает.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationState {
    Enabled,
    Disabled,
}

impl ValidationState {
    pub fn of(mode: &ValidationMode) -> Self {
        if mode.is_enabled() {
            Self::Enabled
        } else {
            Self::Disabled
        }
    }
}

/// Результат наблюдения за потоком рендера.
#[derive(Debug, Clone, PartialEq)]
pub enum RenderObservation {
    /// Поток рендера запущен; снимок его состояния.
    Live {
        status: Box<RenderStatus>,
        validation: ValidationState,
    },
    /// Поток рендера не запущен (GPU или vsync не поднялись). Маршрутизатор работает по запасному
    /// пути: surface подтверждается сразу, рисовать нечем.
    Unavailable { reason: String },
}

/// Источник наблюдений за потоком рендера (на телефоне — дескриптор потока, в тестах — fake).
pub trait RenderStatusSource: Send + Sync {
    fn observe(&self) -> RenderObservation;
}

/// Источник для случая «рендер не поднялся»: всегда `Unavailable` с причиной.
#[derive(Debug, Clone)]
pub struct UnavailableRender {
    reason: String,
}

impl UnavailableRender {
    pub fn new(reason: impl Into<String>) -> Self {
        Self { reason: reason.into() }
    }
}

impl RenderStatusSource for UnavailableRender {
    fn observe(&self) -> RenderObservation {
        RenderObservation::Unavailable {
            reason: self.reason.clone(),
        }
    }
}

/// Формат RAW для диагностики импорта AHardwareBuffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AhbFormat {
    Raw16,
    Raw10,
}

/// Что проверить в `ahb_roundtrip`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AhbCheckRequest {
    pub format: AhbFormat,
    pub width: u32,
    pub height: u32,
}

/// Итог полного прогона `ahb_roundtrip` (нейтральные поля, без типов `gpu-vulkan`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AhbCheckSummary {
    /// Контрольные суммы GPU и CPU совпали (в первом и во втором, кэшированном, импорте).
    pub matched: bool,
    /// Второй импорт того же буфера был попаданием в кэш.
    pub second_import_hit: bool,
    /// Объекты Vulkan после диагностики равны исходным.
    pub live_objects_restored: bool,
    /// Ошибки validation за время прогона (известные отступления сюда не входят).
    pub validation_errors: u64,
    /// Срабатывания известных отступлений (решение 35) за время прогона: VUID -> число.
    pub known_deviations: BTreeMap<String, u64>,
    pub validation: ValidationState,
    /// Однострочное описание отчёта `gpu-vulkan` (формат, шаг строки, суммы…).
    pub details: String,
}

/// Итог проверки импорта: «телефон не умеет» и «сломалось» различаются.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AhbCheckResult {
    Completed(AhbCheckSummary),
    /// `AHardwareBuffer_allocate` отказал для такого формата/usage: данные для отчёта, не сбой.
    AllocationUnsupported {
        status: i32,
        requested: String,
    },
    /// Сбой диагностики (нет Vulkan, ошибка импорта…).
    Failed {
        reason: String,
    },
}

/// Параметры проверки порядка fence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FenceCheckRequest {
    /// Длина цикла работы A (чем больше, тем дольше A).
    pub busy_iterations: u32,
}

/// Итог `fence_order_check` (нейтральные поля).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FenceCheckSummary {
    pub expected_value: u32,
    pub work_a_value: u32,
    pub work_b_value: u32,
    pub b_started_after_a_finished: bool,
    pub work_a_duration_ns: u64,
    pub work_b_duration_ns: u64,
    pub gap_between_a_end_and_b_begin_ns: i64,
    /// `None` — проверить нельзя (нет экспорта fence в sync_fd).
    pub release_fence_pending_at_export: Option<bool>,
    pub exported_fence_was_already_signaled: bool,
    /// Для fence «уже сигнален» ожидающий семафор не создавался.
    pub already_signaled_wait_semaphore_created: bool,
    pub already_signaled_work_completed: bool,
    pub already_signaled_result_value: u32,
    pub validation_errors: u64,
    /// Срабатывания известных отступлений (решение 35): VUID -> число.
    pub known_deviations: BTreeMap<String, u64>,
    pub validation: ValidationState,
}

/// Итог проверки порядка fence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FenceCheckResult {
    Completed(FenceCheckSummary),
    Failed { reason: String },
}

/// Разовые GPU-диагностики. Работают на своём `GpuContext`, отдельном от потока рендера, поэтому
/// не мешают показу кадров; реализация на Vulkan держит этот контекст до конца процесса
/// (почему — `vulkan_diagnostics`). Вызов долгий (десятки и сотни мс) — сценарии зовут его с
/// рабочего потока, а не из маршрутизатора.
pub trait GpuDiagnostics: Send + Sync {
    /// Проверки импорта AHB подряд на одном GPU-контексте; результаты — в порядке запросов.
    /// `on_checked` вызывается после каждой проверки (для учёта прогресса при зависании).
    /// Пачка, а не вызов на формат: контекст не пересоздаётся между форматами (факт сессии 03,
    /// `user/general_problems/general_backlog.md`, п. 2).
    fn ahb_roundtrips(
        &self,
        requests: &[AhbCheckRequest],
        on_checked: &mut dyn FnMut(&AhbCheckRequest),
    ) -> Vec<AhbCheckResult>;
    fn fence_order(&self, request: FenceCheckRequest) -> FenceCheckResult;
}

/// Диагностики, когда GPU недоступен (ПК-сборка без стенда или инициализация провалена).
#[derive(Debug, Clone)]
pub struct UnavailableDiagnostics {
    reason: String,
}

impl UnavailableDiagnostics {
    pub fn new(reason: impl Into<String>) -> Self {
        Self { reason: reason.into() }
    }
}

impl GpuDiagnostics for UnavailableDiagnostics {
    fn ahb_roundtrips(
        &self,
        requests: &[AhbCheckRequest],
        on_checked: &mut dyn FnMut(&AhbCheckRequest),
    ) -> Vec<AhbCheckResult> {
        requests
            .iter()
            .map(|request| {
                on_checked(request);
                AhbCheckResult::Failed {
                    reason: self.reason.clone(),
                }
            })
            .collect()
    }

    fn fence_order(&self, _request: FenceCheckRequest) -> FenceCheckResult {
        FenceCheckResult::Failed {
            reason: self.reason.clone(),
        }
    }
}

/// GPU-сервисы для сценариев: одним объектом, чтобы не раздувать `ScenarioContext`.
#[derive(Clone)]
pub struct GpuServices {
    pub render: Arc<dyn RenderStatusSource>,
    pub diagnostics: Arc<dyn GpuDiagnostics>,
    /// Конфиг потока рендера: сценариям нужны окно пейсинга и желаемая частота.
    pub render_config: RenderLoopConfig,
    /// Куда идут регулировки рендера сценариев (`viewfinder_control`).
    pub(crate) control: Arc<dyn RenderControl>,
    /// Что из рендера тронул текущий сценарий: это вернёт `render_scenario_ended`.
    pub(crate) scenario_claim: Arc<RenderScenarioClaim>,
    /// GPU-контекст стенда насыщения и карта теней устройства (16a, P4b). По умолчанию — «недоступно»;
    /// настоящее подставляет сборка приложения через [`GpuServices::with_saturation`].
    pub saturation: SaturationServices,
}

impl GpuServices {
    /// Сервисы с пустым учётом регулировок (ни один сценарий рендер ещё не трогал).
    pub fn new(
        render: Arc<dyn RenderStatusSource>,
        diagnostics: Arc<dyn GpuDiagnostics>,
        render_config: RenderLoopConfig,
        control: Arc<dyn RenderControl>,
    ) -> Self {
        Self {
            render,
            diagnostics,
            render_config,
            control,
            scenario_claim: Arc::default(),
            saturation: SaturationServices::unavailable("стенд насыщения не подключён к GPU-сервисам"),
        }
    }

    /// Те же сервисы с настоящими источниками стенда насыщения.
    #[must_use]
    pub fn with_saturation(mut self, saturation: SaturationServices) -> Self {
        self.saturation = saturation;
        self
    }
}
