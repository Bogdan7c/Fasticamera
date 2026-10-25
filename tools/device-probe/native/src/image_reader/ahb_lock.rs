//! CPU-доступ к `AHardwareBuffer` (`AHardwareBuffer_lock`) для эталонной контрольной суммы.

use std::ffi::c_void;
use std::ptr;

use ndk_sys::{AHardwareBuffer, AHardwareBuffer_lock, AHardwareBuffer_unlock};

/// `AHARDWAREBUFFER_USAGE_CPU_READ_OFTEN`.
const CPU_READ_OFTEN: u64 = 3;
/// Нет fence: данные кадра уже готовы (GPU дождался acquire-fence, либо мы ждали его на CPU).
const NO_FENCE: i32 = -1;

/// Заблокированный на чтение буфер; `unlock` — в `Drop`.
pub struct LockedBuffer {
    buffer: *mut AHardwareBuffer,
    address: *const u8,
}

impl LockedBuffer {
    /// Блокирует буфер для чтения. Ошибка — текст с кодом `AHardwareBuffer_lock`.
    ///
    /// # Safety
    /// `buffer` должен быть валидным `AHardwareBuffer*`, живущим дольше возвращённого значения
    /// (он принадлежит `AImage`, который мы не освобождаем до конца работы).
    pub unsafe fn lock_for_cpu_read(buffer: *mut AHardwareBuffer) -> Result<Self, String> {
        let mut address: *mut c_void = ptr::null_mut();
        // SAFETY: по контракту функции `buffer` валиден; rect=null — весь буфер; `address` — out.
        let code = unsafe { AHardwareBuffer_lock(buffer, CPU_READ_OFTEN, NO_FENCE, ptr::null(), &mut address) };
        if code != 0 {
            return Err(format!("AHardwareBuffer_lock вернул {code}"));
        }
        if address.is_null() {
            return Err("AHardwareBuffer_lock вернул null-адрес".to_owned());
        }
        Ok(Self {
            buffer,
            address: address.cast(),
        })
    }

    /// Первые `len` байт буфера.
    ///
    /// # Safety
    /// Вызывающий обязан гарантировать, что аллокация содержит минимум `len` байт
    /// (проверяется по `allocationSize` из Vulkan до вызова).
    pub unsafe fn bytes(&self, len: usize) -> &[u8] {
        // SAFETY: адрес валиден, пока буфер заблокирован (живёт `self`); длина — по контракту.
        unsafe { std::slice::from_raw_parts(self.address, len) }
    }
}

impl Drop for LockedBuffer {
    fn drop(&mut self) {
        // SAFETY: буфер был заблокирован в `lock_for_cpu_read`; fence не нужен (чтение).
        let code = unsafe { AHardwareBuffer_unlock(self.buffer, ptr::null_mut()) };
        if code != 0 {
            log::warn!("AHardwareBuffer_unlock вернул {code}");
        }
    }
}
