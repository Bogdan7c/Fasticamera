//! RAII-обёртки над `ACameraManager` / `ACameraMetadata`. Единственное место с unsafe
//! в Camera2-части: остальной код работает с безопасными типами.

use std::ffi::{CStr, CString, c_int, c_void};
use std::ptr::{self, NonNull};

use ndk_sys::{
    ACameraDevice, ACameraDevice_StateCallbacks, ACameraDevice_close, ACameraIdList, ACameraManager,
    ACameraManager_create, ACameraManager_delete, ACameraManager_deleteCameraIdList,
    ACameraManager_getCameraCharacteristics, ACameraManager_getCameraIdList, ACameraManager_openCamera,
    ACameraMetadata, ACameraMetadata_const_entry, ACameraMetadata_free, ACameraMetadata_getConstEntry, camera_status_t,
};

/// Числовой `camera_status_t` (0 == `ACAMERA_OK`).
pub type StatusCode = i32;

const STATUS_OK: StatusCode = 0;

/// Превращает статус в `Result`: `Ok(())` или код ошибки.
fn check(status: camera_status_t) -> Result<(), StatusCode> {
    if status.0 == STATUS_OK { Ok(()) } else { Err(status.0) }
}

/// Владеет `ACameraManager*`; освобождает в `Drop`.
pub struct CameraManager {
    raw: NonNull<ACameraManager>,
}

impl CameraManager {
    /// `ACameraManager_create`. `None`, если сервис камеры недоступен.
    pub fn create() -> Option<Self> {
        // SAFETY: функция не принимает указателей; результат проверяется на null.
        let raw = unsafe { ACameraManager_create() };
        NonNull::new(raw).map(|raw| Self { raw })
    }

    /// Список id камер из `ACameraManager_getCameraIdList`.
    pub fn list_ids(&self) -> Result<Vec<String>, StatusCode> {
        let mut list: *mut ACameraIdList = ptr::null_mut();
        // SAFETY: `raw` жив (владеем им), `list` — валидный out-параметр.
        check(unsafe { ACameraManager_getCameraIdList(self.raw.as_ptr(), &mut list) })?;
        let Some(list_ref) = NonNull::new(list) else {
            return Err(-10000); // ACAMERA_ERROR_UNKNOWN: OK, но список не вернули
        };
        // SAFETY: при успехе `list` указывает на валидную структуру, внутри
        // `numCameras` указателей на NUL-терминированные строки; они живут до deleteCameraIdList.
        let ids = unsafe {
            let list = list_ref.as_ref();
            let count = usize::try_from(list.numCameras).unwrap_or(0);
            (0..count)
                .map(|index| {
                    let id_ptr = *list.cameraIds.add(index);
                    CStr::from_ptr(id_ptr).to_string_lossy().into_owned()
                })
                .collect()
        };
        // SAFETY: `list` получен из getCameraIdList и ещё не освобождён; строки уже скопированы.
        unsafe { ACameraManager_deleteCameraIdList(list_ref.as_ptr()) };
        Ok(ids)
    }

    /// `ACameraManager_getCameraCharacteristics` для произвольного id (даже не из списка).
    pub fn characteristics(&self, camera_id: &str) -> Result<Characteristics, StatusCode> {
        let id = CString::new(camera_id).map_err(|_| -10001)?; // INVALID_PARAMETER
        let mut metadata: *mut ACameraMetadata = ptr::null_mut();
        // SAFETY: `id` — NUL-терминированная строка на время вызова, `metadata` — out-параметр.
        check(unsafe { ACameraManager_getCameraCharacteristics(self.raw.as_ptr(), id.as_ptr(), &mut metadata) })?;
        NonNull::new(metadata).map(|raw| Characteristics { raw }).ok_or(-10000)
    }

    /// Пробует открыть камеру и сразу закрывает. Возвращает статус открытия.
    pub fn probe_open(&self, camera_id: &str) -> StatusCode {
        let Ok(id) = CString::new(camera_id) else {
            return -10001;
        };
        let mut callbacks = ACameraDevice_StateCallbacks {
            context: ptr::null_mut(),
            onDisconnected: Some(on_disconnected),
            onError: Some(on_error),
        };
        let mut device: *mut ACameraDevice = ptr::null_mut();
        // SAFETY: `id` и `callbacks` живут до конца вызова (openCamera копирует колбэки),
        // `device` — валидный out-параметр.
        let status = unsafe { ACameraManager_openCamera(self.raw.as_ptr(), id.as_ptr(), &mut callbacks, &mut device) };
        if let Some(device) = NonNull::new(device) {
            // SAFETY: устройство только что открыто и принадлежит нам; закрываем ровно один раз.
            let close_status = unsafe { ACameraDevice_close(device.as_ptr()) };
            if close_status.0 != STATUS_OK {
                log::warn!("ACameraDevice_close({camera_id}) -> {}", close_status.0);
            }
        }
        status.0
    }
}

impl Drop for CameraManager {
    fn drop(&mut self) {
        // SAFETY: `raw` создан ACameraManager_create и освобождается ровно один раз.
        unsafe { ACameraManager_delete(self.raw.as_ptr()) };
    }
}

/// Колбэк «камера отключена» — нам не нужен, но NDK не любит null-колбэки.
unsafe extern "C" fn on_disconnected(_context: *mut c_void, _device: *mut ACameraDevice) {}

/// Колбэк ошибки устройства — только лог.
unsafe extern "C" fn on_error(_context: *mut c_void, _device: *mut ACameraDevice, error: c_int) {
    log::warn!("ACameraDevice onError({error}) во время probe_open");
}

/// Статические характеристики камеры (`ACameraMetadata`), освобождаются в `Drop`.
pub struct Characteristics {
    raw: NonNull<ACameraMetadata>,
}

impl Characteristics {
    /// Читает запись метаданных; `None`, если тега нет или у него другой тип/пустой.
    fn entry(&self, tag: u32) -> Option<ACameraMetadata_const_entry> {
        // SAFETY: структура — POD с указателями; нули допустимы как начальное значение.
        let mut entry: ACameraMetadata_const_entry = unsafe { std::mem::zeroed() };
        // SAFETY: `raw` жив; `entry` — валидный out-параметр.
        let status = unsafe { ACameraMetadata_getConstEntry(self.raw.as_ptr(), tag, &mut entry) };
        (status.0 == STATUS_OK && entry.count > 0).then_some(entry)
    }

    /// Массив int32 по тегу (типы проверяются по `ACAMERA_TYPE_INT32 = 1`).
    pub fn i32_values(&self, tag: u32) -> Vec<i32> {
        const TYPE_INT32: u8 = 1;
        match self.entry(tag) {
            // SAFETY: при type_ == INT32 union содержит `i32_` на `count` элементов,
            // живущих, пока жив `self.raw` (мы копируем сразу).
            Some(entry) if entry.type_ == TYPE_INT32 => unsafe {
                std::slice::from_raw_parts(entry.data.i32_, entry.count as usize).to_vec()
            },
            _ => Vec::new(),
        }
    }

    /// Массив int64 по тегу (`ACAMERA_TYPE_INT64 = 3`).
    pub fn i64_values(&self, tag: u32) -> Vec<i64> {
        const TYPE_INT64: u8 = 3;
        match self.entry(tag) {
            // SAFETY: см. i32_values; для INT64 активен `i64_`.
            Some(entry) if entry.type_ == TYPE_INT64 => unsafe {
                std::slice::from_raw_parts(entry.data.i64_, entry.count as usize).to_vec()
            },
            _ => Vec::new(),
        }
    }

    /// Массив байт по тегу (`ACAMERA_TYPE_BYTE = 0`).
    pub fn byte_values(&self, tag: u32) -> Vec<u8> {
        const TYPE_BYTE: u8 = 0;
        match self.entry(tag) {
            // SAFETY: см. i32_values; для BYTE активен `u8_`.
            Some(entry) if entry.type_ == TYPE_BYTE => unsafe {
                std::slice::from_raw_parts(entry.data.u8_, entry.count as usize).to_vec()
            },
            _ => Vec::new(),
        }
    }
}

impl Drop for Characteristics {
    fn drop(&mut self) {
        // SAFETY: `raw` получен от getCameraCharacteristics, освобождается один раз.
        unsafe { ACameraMetadata_free(self.raw.as_ptr()) };
    }
}
