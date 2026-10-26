//! Validation layer: режим, счётчики ошибок и messenger `VK_EXT_debug_utils` → `tracing`.
//!
//! Любое сообщение слоя уровня ERROR увеличивает счётчик и запоминает первое сообщение;
//! GPU-тесты в конце проверяют `errors == 0`, а отдельный тест доказывает, что хук живой
//! (намеренная ошибка даёт `errors > 0`) — иначе «0 ошибок» ничего бы не значило.

use std::collections::BTreeMap;
use std::ffi::{CStr, c_void};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use ash::{ext, vk};

use crate::error::{GpuInitError, init_step};

/// Имя слоя Khronos validation.
pub const VALIDATION_LAYER_NAME: &CStr = c"VK_LAYER_KHRONOS_validation";

/// Включать ли проверку синхронизации (synchronization validation) внутри слоя.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncValidation {
    /// Проверять барьеры и гонки (дороже, но ловит ошибки синхронизации — основа тестов).
    Enabled,
    /// Только базовая проверка API.
    Disabled,
}

/// Одно осознанное отступление от правил Vulkan (решение 35): сообщение слоя с этим VUID не
/// считается ошибкой, а учитывается отдельным счётчиком вместе с причиной.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownValidationDeviation {
    /// Точное имя сообщения слоя (`pMessageIdName`), например
    /// `VUID-VkMemoryAllocateInfo-pNext-02387`. Сравнение — на равенство строк целиком:
    /// префикс или подстрока настоящего VUID НЕ совпадает.
    pub vuid: String,
    /// Почему отступление допустимо (попадает в лог и отчёты).
    pub reason: String,
}

/// Список известных отступлений. Пустой список (по умолчанию) = любая ошибка слоя — провал.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KnownValidationDeviations {
    entries: Vec<KnownValidationDeviation>,
}

impl KnownValidationDeviations {
    /// Пустой список (`const`, чтобы жить в [`ValidationMode::FULL`]).
    pub const fn none() -> Self {
        Self { entries: Vec::new() }
    }

    /// Список из готовых записей.
    pub fn new(entries: Vec<KnownValidationDeviation>) -> Self {
        Self { entries }
    }

    /// Записи списка.
    pub fn entries(&self) -> &[KnownValidationDeviation] {
        &self.entries
    }

    /// Запись для сообщения слоя с таким именем (точное совпадение) или `None`.
    fn find(&self, message_id_name: &str) -> Option<(usize, &KnownValidationDeviation)> {
        self.entries
            .iter()
            .enumerate()
            .find(|(_, entry)| entry.vuid == message_id_name)
    }
}

/// Режим validation для контекста.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationMode {
    /// Слой не подключается (релиз).
    Disabled,
    /// Слой подключается; нет слоя в системе — ошибка `ValidationLayerUnavailable`.
    Enabled {
        /// Включать ли sync-валидацию.
        synchronization: SyncValidation,
        /// Известные отступления (решение 35): их VUID считаются отдельно и тест не валят.
        known_deviations: KnownValidationDeviations,
    },
}

impl ValidationMode {
    /// Полная проверка: слой + синхронизация, без известных отступлений (режим GPU-тестов).
    pub const FULL: ValidationMode = ValidationMode::Enabled {
        synchronization: SyncValidation::Enabled,
        known_deviations: KnownValidationDeviations::none(),
    };

    /// Полная проверка с заданным списком известных отступлений (режим сценариев приложения).
    pub fn full_with_known_deviations(known_deviations: KnownValidationDeviations) -> Self {
        ValidationMode::Enabled {
            synchronization: SyncValidation::Enabled,
            known_deviations,
        }
    }

    /// Известные отступления режима (у `Disabled` — пустой список).
    pub fn known_deviations(&self) -> KnownValidationDeviations {
        match self {
            ValidationMode::Disabled => KnownValidationDeviations::none(),
            ValidationMode::Enabled { known_deviations, .. } => known_deviations.clone(),
        }
    }

    /// Подключается ли слой вообще.
    pub fn is_enabled(&self) -> bool {
        matches!(self, ValidationMode::Enabled { .. })
    }

    /// Запрошена ли sync-валидация.
    pub fn wants_synchronization(&self) -> bool {
        matches!(
            self,
            ValidationMode::Enabled {
                synchronization: SyncValidation::Enabled,
                ..
            }
        )
    }
}

/// Отчёт о сообщениях validation за время жизни контекста.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ValidationReport {
    /// Число сообщений уровня ERROR.
    pub errors: u64,
    /// Число сообщений уровня WARNING (включая предупреждения о производительности).
    pub warnings: u64,
    /// Текст первого ERROR (для диагностики провала теста).
    pub first_error: Option<String>,
    /// Сколько раз сработал каждый VUID из списка известных отступлений (решение 35).
    /// Ключ есть для КАЖДОГО перечисленного VUID, даже если он не сработал (значение 0).
    /// Эти сообщения в `errors` не входят.
    pub known_deviations: BTreeMap<String, u64>,
}

impl ValidationReport {
    /// Сумма срабатываний всех известных отступлений.
    pub fn known_deviation_total(&self) -> u64 {
        self.known_deviations.values().sum()
    }

    /// Сколько раз каждое известное отступление сработало ПОСЛЕ отчёта `earlier` (разность
    /// счётчиков; ключи — все VUID этого отчёта). Счётчики только растут, вычитание насыщающее.
    pub fn known_deviations_since(&self, earlier: &ValidationReport) -> BTreeMap<String, u64> {
        self.known_deviations
            .iter()
            .map(|(vuid, count_now)| {
                let count_before = earlier.known_deviations.get(vuid).copied().unwrap_or(0);
                (vuid.clone(), count_now.saturating_sub(count_before))
            })
            .collect()
    }
}

/// Как учтено очередное сообщение слоя.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordedMessage<'deviation> {
    /// Ошибка (увеличила `errors`).
    Error,
    /// Известное отступление (увеличило счётчик VUID, причина — внутри).
    KnownDeviation { reason: &'deviation str },
    /// Предупреждение.
    Warning,
    /// Информационное сообщение (не считается).
    Info,
}

/// Общие счётчики; на них указывает `pUserData` messenger'а.
#[derive(Debug, Default)]
pub(crate) struct ValidationCounters {
    errors: AtomicU64,
    warnings: AtomicU64,
    first_error: Mutex<Option<String>>,
    /// Известные отступления (решение 35).
    known: KnownValidationDeviations,
    /// Счётчики срабатываний; индекс совпадает с индексом записи в `known`.
    known_hits: Vec<AtomicU64>,
}

impl ValidationCounters {
    /// Счётчики с заданным списком известных отступлений.
    pub(crate) fn new(known: KnownValidationDeviations) -> Self {
        let known_hits = known.entries().iter().map(|_| AtomicU64::new(0)).collect();
        Self {
            known,
            known_hits,
            ..Self::default()
        }
    }

    /// Учитывает одно сообщение слоя. `message_id_name` — `pMessageIdName` (VUID); отступления
    /// ищутся только среди ERROR и только по точному совпадению имени.
    pub(crate) fn record(
        &self,
        severity: vk::DebugUtilsMessageSeverityFlagsEXT,
        message_id_name: &str,
        text: &str,
    ) -> RecordedMessage<'_> {
        if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) {
            if let Some((index, deviation)) = self.known.find(message_id_name) {
                if let Some(hits) = self.known_hits.get(index) {
                    hits.fetch_add(1, Ordering::Relaxed);
                }
                return RecordedMessage::KnownDeviation {
                    reason: &deviation.reason,
                };
            }
            self.errors.fetch_add(1, Ordering::Relaxed);
            // Замок отравлен только паникой в другом потоке; данные (Option<String>) остаются целыми.
            let mut first_error = self.first_error.lock().unwrap_or_else(PoisonError::into_inner);
            if first_error.is_none() {
                *first_error = Some(text.to_owned());
            }
            RecordedMessage::Error
        } else if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::WARNING) {
            self.warnings.fetch_add(1, Ordering::Relaxed);
            RecordedMessage::Warning
        } else {
            RecordedMessage::Info
        }
    }

    /// Текущий отчёт.
    pub(crate) fn report(&self) -> ValidationReport {
        ValidationReport {
            errors: self.errors.load(Ordering::Relaxed),
            warnings: self.warnings.load(Ordering::Relaxed),
            first_error: self.first_error.lock().unwrap_or_else(PoisonError::into_inner).clone(),
            known_deviations: self
                .known
                .entries()
                .iter()
                .zip(&self.known_hits)
                .map(|(entry, hits)| (entry.vuid.clone(), hits.load(Ordering::Relaxed)))
                .collect(),
        }
    }
}

/// Параметры messenger'а: все важности и типы сообщений слоя.
pub(crate) fn messenger_create_info(user_data: *mut c_void) -> vk::DebugUtilsMessengerCreateInfoEXT<'static> {
    vk::DebugUtilsMessengerCreateInfoEXT::default()
        .message_severity(
            vk::DebugUtilsMessageSeverityFlagsEXT::ERROR
                | vk::DebugUtilsMessageSeverityFlagsEXT::WARNING
                | vk::DebugUtilsMessageSeverityFlagsEXT::INFO,
        )
        .message_type(
            vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
        )
        .pfn_user_callback(Some(debug_callback))
        .user_data(user_data)
}

/// Callback слоя. Не паникует и не вызывает ничего, что может упасть.
///
/// # Safety
/// Вызывается только слоем Vulkan; `user_data` — указатель из [`ValidationHook::user_data`],
/// пока messenger жив, `callback_data` валиден на время вызова.
unsafe extern "system" fn debug_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _message_types: vk::DebugUtilsMessageTypeFlagsEXT,
    callback_data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    user_data: *mut c_void,
) -> vk::Bool32 {
    let mut message_id_name = String::new();
    let text = if callback_data.is_null() {
        "<пустой callback_data>".to_owned()
    } else {
        // SAFETY: слой гарантирует валидность структуры на время вызова.
        let data = unsafe { &*callback_data };
        let id_name = if data.p_message_id_name.is_null() {
            ""
        } else {
            // SAFETY: непустой указатель на C-строку, валидную на время вызова.
            unsafe { CStr::from_ptr(data.p_message_id_name) }.to_str().unwrap_or("")
        };
        let message = if data.p_message.is_null() {
            ""
        } else {
            // SAFETY: то же — C-строка слоя, валидная на время вызова.
            unsafe { CStr::from_ptr(data.p_message) }
                .to_str()
                .unwrap_or("<не UTF-8>")
        };
        id_name.clone_into(&mut message_id_name);
        format!("[{id_name}] {message}")
    };

    // Учёт идёт ДО логирования: от него зависит, ошибка это или известное отступление.
    let recorded = if user_data.is_null() {
        None
    } else {
        // SAFETY: user_data создан из `Arc::into_raw` в `ValidationHook::new` и живёт, пока жив hook
        // (hook уничтожает messenger до освобождения Arc).
        let counters = unsafe { &*(user_data as *const ValidationCounters) };
        Some(counters.record(severity, &message_id_name, &text))
    };

    match recorded {
        Some(RecordedMessage::KnownDeviation { reason }) => {
            tracing::warn!(target: "fasticamera::gpu::validation", "известное отступление (решение 35): {text}; причина: {reason}");
        }
        _ if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) => {
            tracing::error!(target: "fasticamera::gpu::validation", "{text}");
        }
        _ if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::WARNING) => {
            tracing::warn!(target: "fasticamera::gpu::validation", "{text}");
        }
        _ => tracing::debug!(target: "fasticamera::gpu::validation", "{text}"),
    }
    // FALSE: вызов, вызвавший сообщение, не прерывается.
    vk::FALSE
}

/// Владелец указателя `pUserData`: держит счётчики живыми, пока на них может ссылаться слой.
pub(crate) struct ValidationHook {
    counters: Arc<ValidationCounters>,
    user_data: *const ValidationCounters,
}

// SAFETY: сырой указатель — это `Arc::into_raw` на потокобезопасный `ValidationCounters`
// (атомики + Mutex); владеет им только hook.
unsafe impl Send for ValidationHook {}
// SAFETY: см. выше; наружу выдаётся только неизменяемый доступ.
unsafe impl Sync for ValidationHook {}

impl ValidationHook {
    /// Создаёт hook со свежими счётчиками и списком известных отступлений.
    pub(crate) fn new(known_deviations: KnownValidationDeviations) -> Self {
        let counters = Arc::new(ValidationCounters::new(known_deviations));
        let user_data = Arc::into_raw(Arc::clone(&counters));
        Self { counters, user_data }
    }

    /// Указатель для `pUserData` (instance create-info и messenger).
    pub(crate) fn user_data(&self) -> *mut c_void {
        self.user_data as *mut c_void
    }

    /// Отчёт по счётчикам.
    pub(crate) fn report(&self) -> ValidationReport {
        self.counters.report()
    }
}

impl Drop for ValidationHook {
    fn drop(&mut self) {
        // SAFETY: возвращаем ссылку, отданную `Arc::into_raw` в `new`; к этому моменту messenger
        // уже уничтожен (контекст дропает его раньше), callback больше не вызовется.
        drop(unsafe { Arc::from_raw(self.user_data) });
    }
}

/// Живой messenger `VK_EXT_debug_utils`. Уничтожается явно до уничтожения instance.
pub(crate) struct DebugMessenger {
    loader: ext::debug_utils::Instance,
    handle: vk::DebugUtilsMessengerEXT,
}

impl DebugMessenger {
    /// Создаёт messenger поверх уже созданного instance.
    pub(crate) fn create(
        entry: &ash::Entry,
        instance: &ash::Instance,
        hook: &ValidationHook,
    ) -> Result<Self, GpuInitError> {
        let loader = ext::debug_utils::Instance::new(entry, instance);
        let create_info = messenger_create_info(hook.user_data());
        // SAFETY: create_info ссылается на живой hook (его user_data), instance валиден.
        let handle = init_step(
            unsafe { loader.create_debug_utils_messenger(&create_info, None) },
            "vkCreateDebugUtilsMessengerEXT",
        )?;
        Ok(Self { loader, handle })
    }

    /// Уничтожает messenger. Вызывается ровно один раз до `destroy_instance`.
    pub(crate) fn destroy(self) {
        // SAFETY: handle создан этим загрузчиком; instance ещё жив (порядок задаёт контекст).
        unsafe {
            self.loader.destroy_debug_utils_messenger(self.handle, None);
        }
    }
}

#[cfg(test)]
mod tests;
