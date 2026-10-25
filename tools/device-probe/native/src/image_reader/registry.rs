//! Реестр `AImageReader`. Наружу (Kotlin) отдаются только монотонные i64-handle,
//! никогда не указатели: так устаревший handle не приведёт к use-after-free.
//!
//! Блокировки:
//! * глобальный мьютекс реестра держится только на время поиска/вставки;
//! * у каждого ридера свой мьютекс на нативный указатель. Долгий прогон импорта держит
//!   его целиком, поэтому параллельный `destroy` просто дождётся конца прогона, а не
//!   удалит ридер из-под работающего GPU-кода.

use std::collections::HashMap;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};

use ndk_sys::{AImageReader, AImageReader_delete, AImageReader_newWithUsage};

use crate::error::ProbeError;
use crate::report::{ReaderCreateFailed, ReaderCreated, ReaderDestroyed};
use crate::status_names::media_status_name;

/// Владеет `AImageReader*`. Удаление — в `Drop`.
pub struct NativeReader {
    raw: NonNull<AImageReader>,
}

// SAFETY: AImageReader потокобезопасен по документации NDK (acquire/delete можно звать
// из разных потоков); доступ в любом случае сериализован мьютексом `ReaderEntry`.
unsafe impl Send for NativeReader {}

impl NativeReader {
    /// Сырой указатель для вызовов NDK. Валиден, пока жив `NativeReader`.
    pub fn as_ptr(&self) -> *mut AImageReader {
        self.raw.as_ptr()
    }
}

impl Drop for NativeReader {
    fn drop(&mut self) {
        // SAFETY: указатель создан AImageReader_newWithUsage и удаляется ровно один раз.
        // Все AImage, взятые из ридера, к этому моменту должны быть освобождены
        // (их живёт `AcquiredImage`, ограниченный временем прогона под мьютексом).
        unsafe { AImageReader_delete(self.raw.as_ptr()) };
    }
}

/// Запись реестра: параметры создания + защищённый нативный ридер (`None` после destroy).
pub struct ReaderEntry {
    /// Формат `AIMAGE_FORMAT_*`, с которым создан ридер (нужен для геометрии строк).
    pub format: i32,
    native: Mutex<Option<NativeReader>>,
}

impl ReaderEntry {
    /// Блокирует ридер на время работы. `None` внутри — ридер уже уничтожен.
    pub fn lock_native(&self) -> MutexGuard<'_, Option<NativeReader>> {
        self.native.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

static READERS: LazyLock<Mutex<HashMap<i64, Arc<ReaderEntry>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// Следующий handle. Начинаем с 1: 0 удобно считать «нет ридера» на стороне Kotlin.
static NEXT_HANDLE: AtomicI64 = AtomicI64::new(1);

fn readers() -> MutexGuard<'static, HashMap<i64, Arc<ReaderEntry>>> {
    READERS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Находит запись по handle.
pub fn find_reader(handle: i64) -> Option<Arc<ReaderEntry>> {
    readers().get(&handle).cloned()
}

/// Параметры создания ридера.
pub struct ReaderParams {
    pub width: i32,
    pub height: i32,
    pub format: i32,
    pub usage: u64,
    pub max_images: i32,
}

/// `AImageReader_newWithUsage` + регистрация. Ошибка — код `media_status_t`.
fn create_native(params: &ReaderParams) -> Result<NativeReader, i32> {
    let mut raw: *mut AImageReader = ptr::null_mut();
    // SAFETY: `raw` — валидный out-параметр; остальные аргументы — числа.
    let status = unsafe {
        AImageReader_newWithUsage(
            params.width,
            params.height,
            params.format,
            params.usage,
            params.max_images,
            &mut raw,
        )
    };
    if status.0 != 0 {
        return Err(status.0);
    }
    NonNull::new(raw).map(|raw| NativeReader { raw }).ok_or(-10000) // AMEDIA_ERROR_UNKNOWN: OK, но указатель null
}

/// JSON для `rawReaderCreate`.
pub fn create_reader_json(params: ReaderParams) -> Result<String, ProbeError> {
    match create_native(&params) {
        Ok(native) => {
            let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
            let entry = ReaderEntry {
                format: params.format,
                native: Mutex::new(Some(native)),
            };
            readers().insert(handle, Arc::new(entry));
            Ok(serde_json::to_string(&ReaderCreated {
                ok: true,
                handle,
                media_status: media_status_name(0),
            })?)
        }
        Err(status) => Ok(serde_json::to_string(&ReaderCreateFailed {
            ok: false,
            media_status: media_status_name(status),
            media_status_code: status,
            width: params.width,
            height: params.height,
            format: params.format,
            usage: params.usage,
            max_images: params.max_images,
        })?),
    }
}

/// JSON для `rawReaderDestroy`. Неизвестный handle — штатный `unknown_handle`.
pub fn destroy_reader_json(handle: i64) -> Result<String, ProbeError> {
    let Some(entry) = readers().remove(&handle) else {
        return Ok(r#"{"ok":false,"error":"unknown_handle"}"#.to_owned());
    };
    // Ждём окончания возможного прогона, затем удаляем нативный ридер (Drop).
    drop(entry.lock_native().take());
    Ok(serde_json::to_string(&ReaderDestroyed { ok: true })?)
}
