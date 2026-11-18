//! `ACaptureRequest`: сборка запроса из плана и запись значений.

use std::ffi::c_void;
use std::ptr::{self, NonNull};

use camera_core::backend::BackendFailure;
use camera_ndk_sys as ndk;

use crate::entry::EntryValue;
use crate::request_origin::RequestKey;
use crate::request_plan::RequestTemplate;
use crate::status_mapping::camera_backend_failure;
use crate::tags;
use crate::vendor::RequestEntryWriter;

use super::device::OpenedDevice;
use super::outputs::OutputTarget;

/// Владелец `ACaptureRequest`. Освобождается после отправки: NDK копирует запрос при постановке.
pub(super) struct CaptureRequestHandle {
    raw: NonNull<ndk::ACaptureRequest>,
}

impl CaptureRequestHandle {
    pub(super) fn create(device: &OpenedDevice, template: RequestTemplate) -> Result<Self, BackendFailure> {
        let mut raw: *mut ndk::ACaptureRequest = ptr::null_mut();
        // SAFETY: устройство открыто и живо, `raw` — валидное выходное место.
        let status =
            unsafe { ndk::ACameraDevice_createCaptureRequest(device.as_ptr(), template.ndk_value(), &mut raw) };
        match NonNull::new(raw) {
            Some(raw) if status == tags::STATUS_OK => Ok(Self { raw }),
            _ => Err(camera_backend_failure("ACameraDevice_createCaptureRequest", status)),
        }
    }

    pub(super) fn as_ptr(&self) -> *mut ndk::ACaptureRequest {
        self.raw.as_ptr()
    }

    /// Помечает запрос ключом происхождения (`ACaptureRequest_setUserContext`). Метку надо ставить
    /// **до** отправки: после отправки контекст в колбэках уже не меняется (заголовок NDK).
    ///
    /// Ключ — число в роли указателя: NDK хранит и возвращает его, но никто его не разыменовывает,
    /// поэтому висячего указателя быть не может (см. `request_origin`).
    pub(super) fn tag_origin(&mut self, key: RequestKey) -> Result<(), BackendFailure> {
        let context = ptr::without_provenance_mut::<c_void>(key.as_raw());
        // SAFETY: запрос жив; контекст не разыменовывается ни NDK, ни нами — это просто число.
        let status = unsafe { ndk::ACaptureRequest_setUserContext(self.raw.as_ptr(), context) };
        if status == tags::STATUS_OK {
            Ok(())
        } else {
            Err(camera_backend_failure("ACaptureRequest_setUserContext", status))
        }
    }

    /// Добавляет цель (окно приёмника). Цель должна пережить запрос — её держит сессия.
    pub(super) fn add_target(&mut self, target: &OutputTarget) -> Result<(), BackendFailure> {
        // SAFETY: запрос и цель живы; цель живёт дольше запроса (владелец — `ActiveSession`).
        let status = unsafe { ndk::ACaptureRequest_addTarget(self.raw.as_ptr(), target.as_ptr()) };
        if status == tags::STATUS_OK {
            Ok(())
        } else {
            Err(camera_backend_failure("ACaptureRequest_addTarget", status))
        }
    }
}

impl Drop for CaptureRequestHandle {
    fn drop(&mut self) {
        // SAFETY: запрос создан `createCaptureRequest` и принадлежит только нам.
        unsafe { ndk::ACaptureRequest_free(self.raw.as_ptr()) };
    }
}

impl RequestEntryWriter for CaptureRequestHandle {
    fn write_entry(&mut self, tag: u32, value: &EntryValue) -> Result<(), i32> {
        let request = self.raw.as_ptr();
        let count = u32::try_from(value.len()).map_err(|_| tags::STATUS_INVALID_PARAMETER)?;
        if count == 0 {
            return Err(tags::STATUS_INVALID_PARAMETER);
        }
        // SAFETY: запрос жив; указатель на данные и `count` берутся из одного и того же вектора,
        // который живёт до конца вызова (NDK копирует значения внутрь запроса).
        let status = unsafe {
            match value {
                EntryValue::Bytes(values) => ndk::ACaptureRequest_setEntry_u8(request, tag, count, values.as_ptr()),
                EntryValue::Int32s(values) => ndk::ACaptureRequest_setEntry_i32(request, tag, count, values.as_ptr()),
                EntryValue::Int64s(values) => ndk::ACaptureRequest_setEntry_i64(request, tag, count, values.as_ptr()),
                EntryValue::Floats(values) => ndk::ACaptureRequest_setEntry_float(request, tag, count, values.as_ptr()),
                EntryValue::Doubles(values) => {
                    ndk::ACaptureRequest_setEntry_double(request, tag, count, values.as_ptr())
                }
                EntryValue::Rationals(values) => {
                    let converted: Vec<ndk::ACameraMetadata_rational> = values
                        .iter()
                        .map(|rational| ndk::ACameraMetadata_rational {
                            numerator: rational.numerator,
                            denominator: rational.denominator,
                        })
                        .collect();
                    ndk::ACaptureRequest_setEntry_rational(request, tag, count, converted.as_ptr())
                }
            }
        };
        if status == tags::STATUS_OK { Ok(()) } else { Err(status) }
    }
}
