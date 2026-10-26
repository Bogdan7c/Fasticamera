//! `ACameraManager` и метаданные камеры: RAII-владельцы и чтение записей.
//!
//! * [`CameraManager`] — владелец `ACameraManager`; живёт на потоке камеры.
//! * [`Characteristics`] — владелец `ACameraMetadata` характеристик одной камеры.
//! * [`MetadataView`] — заимствованный взгляд на `ACameraMetadata` (характеристики или результат
//!   запроса из колбэка); реализует нейтральные [`MetadataSource`] и [`TagResolver`].

use std::ffi::{CStr, CString};
use std::marker::PhantomData;
use std::ptr::{self, NonNull};

use camera_contract::capabilities::Rational;
use camera_core::backend::BackendFailure;
use camera_ndk_sys as ndk;

use crate::entry::{EntryLookup, EntryValue, MetadataSource};
use crate::status_mapping::camera_backend_failure;
use crate::tags::{self, Tag};
use crate::vendor::{TagResolution, TagResolver};

/// Владелец `ACameraManager`.
pub(super) struct CameraManager {
    raw: NonNull<ndk::ACameraManager>,
}

impl CameraManager {
    pub(super) fn create() -> Result<Self, BackendFailure> {
        // SAFETY: `ACameraManager_create` не имеет предусловий; результат — владеющий указатель или null.
        let raw = unsafe { ndk::ACameraManager_create() };
        NonNull::new(raw).map(|raw| Self { raw }).ok_or_else(|| BackendFailure {
            operation: "ACameraManager_create",
            code: 0,
            detail: "вернул null".to_owned(),
        })
    }

    pub(super) fn as_ptr(&self) -> *mut ndk::ACameraManager {
        self.raw.as_ptr()
    }

    /// ID всех камер, видимых приложению (скрытые фреймворком камеры сюда не попадают).
    pub(super) fn camera_ids(&self) -> Result<Vec<String>, BackendFailure> {
        let mut list: *mut ndk::ACameraIdList = ptr::null_mut();
        // SAFETY: менеджер жив; `list` — валидное место для выходного указателя.
        let status = unsafe { ndk::ACameraManager_getCameraIdList(self.raw.as_ptr(), &mut list) };
        if status != tags::STATUS_OK || list.is_null() {
            return Err(camera_backend_failure("ACameraManager_getCameraIdList", status));
        }
        // SAFETY: `list` выдан NDK и жив до `ACameraManager_deleteCameraIdList` ниже.
        let ids = unsafe { copy_id_list(list) };
        // SAFETY: список получен из `getCameraIdList` и больше не используется.
        unsafe { ndk::ACameraManager_deleteCameraIdList(list) };
        Ok(ids)
    }

    /// Характеристики камеры.
    pub(super) fn characteristics(&self, camera_id: &str) -> Result<Characteristics, BackendFailure> {
        let c_id = CString::new(camera_id).map_err(|_| BackendFailure {
            operation: "ACameraManager_getCameraCharacteristics",
            code: 0,
            detail: format!("ID камеры {camera_id:?} содержит NUL"),
        })?;
        let mut raw: *mut ndk::ACameraMetadata = ptr::null_mut();
        // SAFETY: менеджер жив, `c_id` — NUL-терминированная строка, `raw` — валидное выходное место.
        let status =
            unsafe { ndk::ACameraManager_getCameraCharacteristics(self.raw.as_ptr(), c_id.as_ptr(), &mut raw) };
        match NonNull::new(raw) {
            Some(raw) if status == tags::STATUS_OK => Ok(Characteristics { raw }),
            _ => Err(camera_backend_failure(
                "ACameraManager_getCameraCharacteristics",
                status,
            )),
        }
    }
}

impl Drop for CameraManager {
    fn drop(&mut self) {
        // SAFETY: менеджер создан `ACameraManager_create` и принадлежит только нам; устройства и
        // колбэки к этому моменту уже закрыты (порядок в `AndroidCameraBackend::drop`).
        unsafe { ndk::ACameraManager_delete(self.raw.as_ptr()) };
    }
}

/// Копирует `ACameraIdList` в строки.
///
/// # Safety
/// `list` — валидный список, выданный `ACameraManager_getCameraIdList` и ещё не удалённый.
unsafe fn copy_id_list(list: *mut ndk::ACameraIdList) -> Vec<String> {
    // SAFETY: по контракту функции указатель валиден.
    let list = unsafe { &*list };
    let count = usize::try_from(list.numCameras).unwrap_or(0);
    if count == 0 || list.cameraIds.is_null() {
        return Vec::new();
    }
    // SAFETY: NDK гарантирует массив из `numCameras` указателей на NUL-терминированные строки.
    let pointers = unsafe { std::slice::from_raw_parts(list.cameraIds, count) };
    pointers
        .iter()
        .filter(|id| !id.is_null())
        // SAFETY: непустой указатель на NUL-терминированную строку внутри живого списка.
        .map(|id| unsafe { CStr::from_ptr(*id) }.to_string_lossy().into_owned())
        .collect()
}

/// Владелец `ACameraMetadata` характеристик.
pub(super) struct Characteristics {
    raw: NonNull<ndk::ACameraMetadata>,
}

impl Characteristics {
    pub(super) fn view(&self) -> MetadataView<'_> {
        MetadataView {
            raw: self.raw.as_ptr().cast_const(),
            borrowed_from: PhantomData,
        }
    }
}

impl Drop for Characteristics {
    fn drop(&mut self) {
        // SAFETY: метаданные выданы `getCameraCharacteristics` и принадлежат только нам.
        unsafe { ndk::ACameraMetadata_free(self.raw.as_ptr()) };
    }
}

/// Заимствованный взгляд на `ACameraMetadata`.
pub(super) struct MetadataView<'a> {
    raw: *const ndk::ACameraMetadata,
    borrowed_from: PhantomData<&'a ndk::ACameraMetadata>,
}

impl MetadataView<'_> {
    /// Взгляд на метаданные из колбэка.
    ///
    /// # Safety
    /// `raw` — null или валидные `ACameraMetadata`, живые, пока жив возвращённый взгляд
    /// (для колбэка — до его возврата).
    pub(super) unsafe fn from_raw(raw: *const ndk::ACameraMetadata) -> Option<Self> {
        (!raw.is_null()).then_some(Self {
            raw,
            borrowed_from: PhantomData,
        })
    }
}

impl MetadataSource for MetadataView<'_> {
    fn entry(&self, tag: Tag) -> EntryLookup {
        let mut entry = ndk::ACameraMetadata_const_entry::default();
        // SAFETY: метаданные живы (см. инвариант `MetadataView`), `entry` — валидное выходное место.
        let status = unsafe { ndk::ACameraMetadata_getConstEntry(self.raw, tag.code, &mut entry) };
        match status {
            tags::STATUS_OK => {
                // SAFETY: после успешного `getConstEntry` данные записи живы, пока живы метаданные.
                unsafe { copy_entry(&entry) }
            }
            tags::STATUS_METADATA_NOT_FOUND => EntryLookup::Absent,
            other => EntryLookup::Unreadable { status: other },
        }
    }
}

impl TagResolver for MetadataView<'_> {
    fn resolve_tag(&self, name: &str) -> TagResolution {
        let Ok(c_name) = CString::new(name) else {
            return TagResolution::NotFound;
        };
        let mut tag = 0_u32;
        // SAFETY: метаданные живы, `c_name` — NUL-терминированная строка, `tag` — валидное выходное место.
        let status = unsafe { ndk::ACameraMetadata_getTagFromName(self.raw, c_name.as_ptr(), &mut tag) };
        match status {
            tags::STATUS_OK => TagResolution::Found { tag },
            // AOSP возвращает `METADATA_NOT_FOUND` на любое неизвестное имя (ACameraMetadata.cpp).
            tags::STATUS_METADATA_NOT_FOUND => TagResolution::NotFound,
            other => TagResolution::Failed { status: other },
        }
    }
}

/// Копирует запись метаданных в собственные массивы.
///
/// # Safety
/// `entry` заполнена успешным `ACameraMetadata_getConstEntry`, метаданные живы.
unsafe fn copy_entry(entry: &ndk::ACameraMetadata_const_entry) -> EntryLookup {
    let count = entry.count as usize;
    let kind = u32::from(entry.type_);
    // Пустая запись: указатель может быть null, копировать нечего.
    if count == 0 {
        return match kind {
            ndk::ACAMERA_TYPE_BYTE => EntryLookup::Present(EntryValue::Bytes(Vec::new())),
            ndk::ACAMERA_TYPE_INT32 => EntryLookup::Present(EntryValue::Int32s(Vec::new())),
            ndk::ACAMERA_TYPE_INT64 => EntryLookup::Present(EntryValue::Int64s(Vec::new())),
            ndk::ACAMERA_TYPE_FLOAT => EntryLookup::Present(EntryValue::Floats(Vec::new())),
            ndk::ACAMERA_TYPE_DOUBLE => EntryLookup::Present(EntryValue::Doubles(Vec::new())),
            ndk::ACAMERA_TYPE_RATIONAL => EntryLookup::Present(EntryValue::Rationals(Vec::new())),
            _ => EntryLookup::Unreadable {
                status: tags::STATUS_UNKNOWN,
            },
        };
    }
    // SAFETY: чтение поля объединения: активный вариант определяет `type_`, и каждая ветка ниже
    // читает ровно тот указатель, который соответствует типу; `count` элементов валидны.
    unsafe {
        match kind {
            ndk::ACAMERA_TYPE_BYTE => copy_slice(entry.data.u8_, count).map(EntryValue::Bytes),
            ndk::ACAMERA_TYPE_INT32 => copy_slice(entry.data.i32_, count).map(EntryValue::Int32s),
            ndk::ACAMERA_TYPE_INT64 => copy_slice(entry.data.i64_, count).map(EntryValue::Int64s),
            ndk::ACAMERA_TYPE_FLOAT => copy_slice(entry.data.f, count).map(EntryValue::Floats),
            ndk::ACAMERA_TYPE_DOUBLE => copy_slice(entry.data.d, count).map(EntryValue::Doubles),
            ndk::ACAMERA_TYPE_RATIONAL => copy_slice(entry.data.r, count).map(|rationals| {
                EntryValue::Rationals(
                    rationals
                        .into_iter()
                        .map(|rational| Rational {
                            numerator: rational.numerator,
                            denominator: rational.denominator,
                        })
                        .collect(),
                )
            }),
            _ => None,
        }
    }
    .map_or(
        EntryLookup::Unreadable {
            status: tags::STATUS_UNKNOWN,
        },
        EntryLookup::Present,
    )
}

/// Копирует `count` элементов; null при ненулевом `count` — `None`.
///
/// # Safety
/// `data` — null или указатель на `count` валидных элементов `T`.
unsafe fn copy_slice<T: Copy>(data: *const T, count: usize) -> Option<Vec<T>> {
    if data.is_null() {
        return None;
    }
    // SAFETY: по контракту функции `data` ведёт на `count` валидных элементов.
    Some(unsafe { std::slice::from_raw_parts(data, count) }.to_vec())
}
