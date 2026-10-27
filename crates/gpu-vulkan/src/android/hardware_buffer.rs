//! Безопасная обёртка над `AHardwareBuffer*`: владение одной ссылкой, описание, id, блокировка CPU.
//!
//! Ссылка считается по правилам NDK (`AHardwareBuffer_acquire` / `_release`). Тип
//! [`HardwareBufferRef`] владеет РОВНО ОДНОЙ ссылкой: освобождает её в `Drop`. Поэтому импорт,
//! хранящий такую ссылку, не даёт буферу умереть, пока Vulkan-память на него ссылается.

use std::ffi::c_void;
use std::ptr::{self, NonNull};

use super::hardware_buffer_ffi::{
    AHardwareBuffer, AHardwareBuffer_acquire, AHardwareBuffer_allocate, AHardwareBuffer_describe,
    AHardwareBuffer_getId, AHardwareBuffer_lock, AHardwareBuffer_release, AHardwareBuffer_unlock, AHardwareBufferDesc,
    USAGE_CPU_WRITE_OFTEN,
};
use crate::hardware_buffer_cache::HardwareBufferId;
use shell_contract::hardware_buffer::BorrowedHardwareBuffer;

/// Ошибки работы с `AHardwareBuffer`. Статус NDK — это errno-подобный код (`0` — успех).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HardwareBufferError {
    /// Передан нулевой указатель.
    #[error("нулевой указатель AHardwareBuffer")]
    NullPointer,
    /// `AHardwareBuffer_allocate` отказал (формат/usage не поддерживаются или нет памяти).
    #[error("AHardwareBuffer_allocate вернул {status}")]
    AllocateFailed {
        /// Код возврата NDK.
        status: i32,
    },
    /// `AHardwareBuffer_getId` недоступен или отказал.
    #[error("AHardwareBuffer_getId вернул {status}")]
    IdUnavailable {
        /// Код возврата NDK.
        status: i32,
    },
    /// `AHardwareBuffer_lock` отказал.
    #[error("AHardwareBuffer_lock вернул {status}")]
    LockFailed {
        /// Код возврата NDK.
        status: i32,
    },
    /// `AHardwareBuffer_lock` вернул успех, но нулевой адрес.
    #[error("AHardwareBuffer_lock вернул нулевой адрес")]
    LockedAddressNull,
    /// `AHardwareBuffer_unlock` отказал.
    #[error("AHardwareBuffer_unlock вернул {status}")]
    UnlockFailed {
        /// Код возврата NDK.
        status: i32,
    },
}

/// Владеемая ссылка на `AHardwareBuffer`.
pub struct HardwareBufferRef {
    pointer: NonNull<AHardwareBuffer>,
}

// SAFETY: `AHardwareBuffer` — счётчик ссылок с атомарными acquire/release (NDK: объект можно
// передавать между потоками); describe/getId только читают неизменяемые поля. Обёртка не даёт
// одновременного изменяемого доступа к содержимому: CPU-доступ идёт через `&mut self`.
// `Sync` намеренно НЕ реализован: lock/unlock одного буфера из двух потоков NDK не допускает.
unsafe impl Send for HardwareBufferRef {}

impl HardwareBufferRef {
    /// Берёт ЧУЖОЙ буфер (например, от `AImage_getHardwareBuffer`): делает собственный `acquire`,
    /// чтобы буфер пережил владельца (в частности, возврат `AImage` камере).
    ///
    /// # Safety
    /// `pointer` должен быть валидным `AHardwareBuffer*` (или нулевым — тогда ошибка) на момент вызова.
    pub unsafe fn from_borrowed(pointer: *mut AHardwareBuffer) -> Result<Self, HardwareBufferError> {
        let pointer = NonNull::new(pointer).ok_or(HardwareBufferError::NullPointer)?;
        // SAFETY: по контракту функции указатель валиден; acquire даёт нам собственную ссылку.
        unsafe { AHardwareBuffer_acquire(pointer.as_ptr()) };
        Ok(Self { pointer })
    }

    /// Безопасный вариант [`Self::from_borrowed`] для буфера, заимствованного у лизинга кадра камеры.
    ///
    /// [`BorrowedHardwareBuffer`] уже доказывает, что `AHardwareBuffer*` жив (владелец-лизинг держит
    /// его, пока заимствование существует), поэтому `unsafe` для вызывающего не нужен. Обёртка берёт
    /// СВОЮ ссылку NDK, так что буфер переживёт и лизинг, и заимствование: импорт в Vulkan держит его,
    /// пока GPU читает кадр.
    pub fn from_borrowed_buffer(buffer: BorrowedHardwareBuffer<'_>) -> Result<Self, HardwareBufferError> {
        // SAFETY: `BorrowedHardwareBuffer` гарантирует живой `AHardwareBuffer*` на время заимствования,
        // а `from_borrowed` делает собственный acquire внутри этого вызова — до того, как заимствование кончится.
        unsafe { Self::from_borrowed(buffer.as_ptr().as_ptr().cast::<AHardwareBuffer>()) }
    }

    /// Выделяет новый буфер. Начальная ссылка (счётчик = 1) принадлежит возвращённому значению.
    pub fn allocate(description: &AHardwareBufferDesc) -> Result<Self, HardwareBufferError> {
        let mut raw_buffer: *mut AHardwareBuffer = ptr::null_mut();
        // SAFETY: description и out-указатель — валидные локальные ссылки на время вызова.
        let status = unsafe { AHardwareBuffer_allocate(description, &mut raw_buffer) };
        if status != 0 {
            return Err(HardwareBufferError::AllocateFailed { status });
        }
        let pointer = NonNull::new(raw_buffer).ok_or(HardwareBufferError::NullPointer)?;
        Ok(Self { pointer })
    }

    /// Сырой указатель (владение остаётся у обёртки). Для `vkImportAndroidHardwareBufferInfoANDROID`.
    pub fn as_ptr(&self) -> *mut AHardwareBuffer {
        self.pointer.as_ptr()
    }

    /// Описание буфера (`AHardwareBuffer_describe`).
    pub fn describe(&self) -> AHardwareBufferDesc {
        let mut description = AHardwareBufferDesc::default();
        // SAFETY: указатель валиден, пока жива обёртка; out-структура локальная.
        unsafe { AHardwareBuffer_describe(self.pointer.as_ptr(), &mut description) };
        description
    }

    /// Системный идентификатор (ключ кэша импортов). API 31+; minSdk приложения 36.
    pub fn id(&self) -> Result<HardwareBufferId, HardwareBufferError> {
        let mut raw_id: u64 = 0;
        // SAFETY: указатель валиден, out-значение локальное.
        let status = unsafe { AHardwareBuffer_getId(self.pointer.as_ptr(), &mut raw_id) };
        if status != 0 {
            return Err(HardwareBufferError::IdUnavailable { status });
        }
        Ok(HardwareBufferId(raw_id))
    }

    /// Блокирует буфер для записи CPU. Пока `CpuWriteMapping` жив, GPU не должен им пользоваться.
    pub fn lock_for_cpu_write(&mut self) -> Result<CpuWriteMapping<'_>, HardwareBufferError> {
        let mut address: *mut c_void = ptr::null_mut();
        // Параметры: fence = -1 («готов сразу», данных GPU в буфере нет); rect = NULL (весь буфер).
        const NO_WAIT_FENCE: i32 = -1;
        // SAFETY: указатель валиден; `&mut self` исключает параллельную блокировку этой обёрткой.
        let status = unsafe {
            AHardwareBuffer_lock(
                self.pointer.as_ptr(),
                USAGE_CPU_WRITE_OFTEN,
                NO_WAIT_FENCE,
                ptr::null(),
                &mut address,
            )
        };
        if status != 0 {
            return Err(HardwareBufferError::LockFailed { status });
        }
        let address = NonNull::new(address.cast::<u8>()).ok_or_else(|| {
            // Блокировка прошла, но адреса нет: снимаем её, чтобы не оставить буфер заблокированным.
            // SAFETY: буфер только что заблокирован нами; fence = NULL — блокирующий unlock.
            let unlock_status = unsafe { AHardwareBuffer_unlock(self.pointer.as_ptr(), ptr::null_mut()) };
            if unlock_status != 0 {
                tracing::warn!(target: "fasticamera::gpu", unlock_status, "unlock после нулевого адреса lock");
            }
            HardwareBufferError::LockedAddressNull
        })?;
        Ok(CpuWriteMapping {
            buffer: self,
            address,
            unlocked: false,
        })
    }
}

impl Clone for HardwareBufferRef {
    /// Ещё одна собственная ссылка на тот же буфер (`AHardwareBuffer_acquire`).
    fn clone(&self) -> Self {
        // SAFETY: указатель валиден, пока жива `self`; acquire даёт новой обёртке свою ссылку.
        unsafe { AHardwareBuffer_acquire(self.pointer.as_ptr()) };
        Self { pointer: self.pointer }
    }
}

impl Drop for HardwareBufferRef {
    fn drop(&mut self) {
        // SAFETY: обёртка владеет ровно одной ссылкой (acquire в from_borrowed или начальная при allocate).
        unsafe { AHardwareBuffer_release(self.pointer.as_ptr()) };
    }
}

impl std::fmt::Debug for HardwareBufferRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "HardwareBufferRef({:?})", self.pointer)
    }
}

/// Блокировка буфера для записи CPU. `unlock` — явный ([`CpuWriteMapping::unlock`]) или в `Drop`.
pub struct CpuWriteMapping<'buffer> {
    buffer: &'buffer mut HardwareBufferRef,
    address: NonNull<u8>,
    unlocked: bool,
}

impl CpuWriteMapping<'_> {
    /// Записываемые байты буфера.
    ///
    /// # Safety
    /// `length` не должен превышать размер аллокации буфера (`allocationSize` из Vulkan или
    /// `stride × height` из `describe`): NDK не сообщает длину отображения, проверить её здесь нельзя.
    pub unsafe fn bytes_mut(&mut self, length: usize) -> &mut [u8] {
        // SAFETY: адрес валиден, пока буфер заблокирован (жив `self`); длину гарантирует вызывающий;
        // `&mut self` исключает второй срез.
        unsafe { std::slice::from_raw_parts_mut(self.address.as_ptr(), length) }
    }

    /// Разблокирует буфер, БЛОКИРУЯ поток до завершения записи (`fence = NULL`): после возврата
    /// содержимое видно GPU без ожидания fence.
    pub fn unlock(mut self) -> Result<(), HardwareBufferError> {
        self.unlock_blocking()
    }

    fn unlock_blocking(&mut self) -> Result<(), HardwareBufferError> {
        self.unlocked = true;
        // SAFETY: буфер заблокирован этой структурой; NULL-fence = синхронное ожидание внутри NDK.
        let status = unsafe { AHardwareBuffer_unlock(self.buffer.pointer.as_ptr(), ptr::null_mut()) };
        if status != 0 {
            return Err(HardwareBufferError::UnlockFailed { status });
        }
        Ok(())
    }
}

impl Drop for CpuWriteMapping<'_> {
    fn drop(&mut self) {
        if !self.unlocked
            && let Err(error) = self.unlock_blocking()
        {
            tracing::warn!(target: "fasticamera::gpu", %error, "unlock AHardwareBuffer в Drop");
        }
    }
}
