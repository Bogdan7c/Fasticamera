//! Заимствованный `AHardwareBuffer` — буфер кадра камеры, который GPU импортирует без копии.
//!
//! Камера (`camera-android`) и GPU (`gpu-vulkan`) не знают друг о друге, но передают один и тот же
//! буфер. Как `BorrowedFd` в `std`, [`BorrowedHardwareBuffer`] — это «указатель + время жизни»:
//! пока заимствование живо, владелец (лизинг кадра) гарантирует, что буфер не освобождён.
//! Чтобы держать буфер дольше, получатель обязан взять свою ссылку NDK
//! (`AHardwareBuffer_acquire` — это делает `gpu-vulkan` при импорте).
//!
//! Модуль нейтральный: внутри только непрозрачный указатель, вызовов NDK здесь нет.

use std::ffi::c_void;
use std::fmt;
use std::marker::PhantomData;
use std::ptr::NonNull;

/// Непрозрачный `AHardwareBuffer*`, действительный на время `'buffer`.
///
/// `Copy`: копия заимствования безопасна — время жизни у неё то же.
#[derive(Clone, Copy)]
pub struct BorrowedHardwareBuffer<'buffer> {
    pointer: NonNull<c_void>,
    lifetime: PhantomData<&'buffer ()>,
}

impl<'buffer> BorrowedHardwareBuffer<'buffer> {
    /// Оборачивает указатель NDK.
    ///
    /// # Safety
    /// `pointer` — живой `AHardwareBuffer*`, и вызывающий держит его (свою ссылку NDK или
    /// объект, который её держит, например `AImage`) не меньше `'buffer`.
    pub unsafe fn from_raw(pointer: NonNull<c_void>) -> Self {
        Self {
            pointer,
            lifetime: PhantomData,
        }
    }

    /// Указатель для вызовов NDK. Действителен только на время `'buffer`.
    pub fn as_ptr(self) -> NonNull<c_void> {
        self.pointer
    }
}

impl fmt::Debug for BorrowedHardwareBuffer<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BorrowedHardwareBuffer")
            .field("pointer", &self.pointer)
            .finish()
    }
}

// SAFETY: `AHardwareBuffer` — объект с атомарным счётчиком ссылок, его функции NDK потокобезопасны
// (документация `hardware_buffer.h`); заимствование лишь переносит указатель между потоками,
// не меняя гарантии времени жизни, которую держит владелец.
unsafe impl Send for BorrowedHardwareBuffer<'_> {}
// SAFETY: то же — через `&BorrowedHardwareBuffer` доступен только копируемый указатель.
unsafe impl Sync for BorrowedHardwareBuffer<'_> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn borrowed_buffer_returns_the_same_pointer() {
        let mut storage = 0_u8;
        let pointer = NonNull::from(&mut storage).cast::<c_void>();
        // SAFETY: в тесте указатель — живая локальная переменная, заимствование её не переживёт.
        let borrowed = unsafe { BorrowedHardwareBuffer::from_raw(pointer) };
        assert_eq!(borrowed.as_ptr(), pointer);
    }
}
