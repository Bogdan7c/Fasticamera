//! Владение `sync_fd`: fence ожидания перед работой GPU и fence освобождения после неё.
//!
//! Камера (NDK) и GPU живут в разных crate-ах, но обмениваются «файловыми дескрипторами
//! синхронизации» (`sync_fd`): acquire-fence кадра камеры («буфер ещё пишется, не читай до
//! сигнала») и release-fence для `AImage_deleteAsync` («GPU ещё читает, не переиспользуй буфер
//! до сигнала»). Потерять такой дескриптор (утечка) или закрыть дважды (закрытие чужого fd,
//! который уже переиспользован ядром) — тихие, трудноуловимые баги. Поэтому дескриптор живёт
//! только внутри этих двух типов, которые владеют `OwnedFd`.
//!
//! Модуль нейтральный (только `std`): `camera-android` передаёт [`GpuWaitFence`] стороне GPU,
//! не зная про `gpu-vulkan`. Импорт в Vulkan и экспорт из него — в `gpu-vulkan::sync`.
//!
//! # Два состояния fence
//! * «уже сигнален» — дескриптора нет вовсе (NDK отдаёт `-1`, когда ждать нечего);
//! * «ждём sync_fd» — дескриптор, принадлежащий типу.
//!
//! # Почему здесь нет ожидания на CPU
//! Ожидание `poll(2)` требует `libc`, а новых зависимостей ради него не берём. Ожидание на CPU
//! выполняет `gpu-vulkan::sync::wait_release_fence_on_cpu` (дескриптор импортируется во
//! `VkFence`); здесь для этого есть только [`GpuReleaseFence::as_sync_fd`].

#[cfg(unix)]
use std::os::fd::{AsFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd};

/// Значение, которым NDK обозначает «fence не нужен, работа уже завершена».
const NDK_NO_FENCE: i32 = -1;

/// Ошибка превращения «сырого» значения fd от NDK во владеющий тип.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SyncFenceError {
    /// Отрицательное значение, кроме `-1`: NDK так не отвечает, значит перед нами мусор.
    /// Ничего не закрывалось и не принималось во владение.
    #[error("недопустимое значение fd от NDK: {raw} (допустимы -1 и неотрицательные)")]
    InvalidNdkFd {
        /// Полученное значение.
        raw: i32,
    },
    /// На этой платформе `sync_fd` не существует (не unix).
    #[error("sync_fd не поддерживается на этой платформе")]
    UnsupportedPlatform,
}

/// Внутреннее состояние fence; общее для обоих публичных типов.
#[derive(Debug)]
enum FenceState {
    /// Ждать нечего.
    AlreadySignaled,
    /// Ждём сигнала по дескриптору, которым владеем.
    #[cfg(unix)]
    SyncFd(OwnedFd),
}

impl FenceState {
    /// Разбирает сырое значение от NDK. Отрицательное, кроме `-1`, отвергается без побочных эффектов.
    ///
    /// # Safety
    /// Неотрицательный `raw` должен быть открытым дескриптором, владение которым
    /// передаётся этому вызову.
    unsafe fn from_raw_ndk(raw: i32) -> Result<Self, SyncFenceError> {
        if raw == NDK_NO_FENCE {
            return Ok(Self::AlreadySignaled);
        }
        if raw < 0 {
            return Err(SyncFenceError::InvalidNdkFd { raw });
        }
        #[cfg(unix)]
        {
            // SAFETY: раз `raw >= 0`, по контракту функции это открытый fd, владение которым
            // нам передали; больше никто его не закроет.
            Ok(Self::SyncFd(unsafe { OwnedFd::from_raw_fd(raw) }))
        }
        #[cfg(not(unix))]
        {
            Err(SyncFenceError::UnsupportedPlatform)
        }
    }

    /// Отдаёт дескриптор вызывающему; `None` — fence уже сигнален.
    #[cfg(unix)]
    fn into_owned_fd(self) -> Option<OwnedFd> {
        match self {
            Self::AlreadySignaled => None,
            Self::SyncFd(fd) => Some(fd),
        }
    }

    /// Отдаёт «сырое» значение для NDK: `-1` у сигналенного, иначе дескриптор (владение уходит).
    fn into_raw_ndk(self) -> i32 {
        match self {
            Self::AlreadySignaled => NDK_NO_FENCE,
            #[cfg(unix)]
            Self::SyncFd(fd) => fd.into_raw_fd(),
        }
    }

    fn is_already_signaled(&self) -> bool {
        matches!(self, Self::AlreadySignaled)
    }

    /// Заимствует дескриптор, не отдавая владение.
    #[cfg(unix)]
    fn as_sync_fd(&self) -> Option<BorrowedFd<'_>> {
        match self {
            Self::AlreadySignaled => None,
            Self::SyncFd(fd) => Some(fd.as_fd()),
        }
    }
}

/// Fence, которого GPU обязан дождаться перед чтением буфера (acquire-fence кадра камеры).
///
/// Владеет дескриптором: закрытие происходит ровно один раз — при `drop`, либо дескриптор
/// уходит наружу через [`into_owned_fd`](Self::into_owned_fd) (тогда закрывает новый владелец).
#[derive(Debug)]
pub struct GpuWaitFence {
    state: FenceState,
}

impl GpuWaitFence {
    /// Fence, уже сигнальный: ждать нечего.
    pub fn already_signaled() -> Self {
        Self {
            state: FenceState::AlreadySignaled,
        }
    }

    /// Берёт во владение настоящий `sync_fd`.
    #[cfg(unix)]
    pub fn from_sync_fd(fd: OwnedFd) -> Self {
        Self {
            state: FenceState::SyncFd(fd),
        }
    }

    /// Принимает значение fd так, как его отдаёт NDK (например, `AImageReader_acquireNextImage`).
    ///
    /// `-1` означает «уже сигнален»; другое отрицательное значение — ошибка
    /// [`SyncFenceError::InvalidNdkFd`] (дескриптор не принимается).
    ///
    /// # Safety
    /// Если `raw >= 0`, это должен быть открытый дескриптор, который больше никто не закроет:
    /// владение им **переходит** этому вызову (при успехе — возвращённому fence, при ошибке
    /// неотрицательных значений не бывает). Вызывающий не должен использовать `raw` после вызова.
    pub unsafe fn from_raw_ndk_fd(raw: i32) -> Result<Self, SyncFenceError> {
        // SAFETY: контракт полностью передан вызывающему (см. раздел Safety).
        let state = unsafe { FenceState::from_raw_ndk(raw) }?;
        Ok(Self { state })
    }

    /// Fence уже сигнален (ждать нечего)?
    pub fn is_already_signaled(&self) -> bool {
        self.state.is_already_signaled()
    }

    /// Отдаёт дескриптор новому владельцу; `None` — fence был уже сигнален.
    #[cfg(unix)]
    pub fn into_owned_fd(self) -> Option<OwnedFd> {
        self.state.into_owned_fd()
    }

    /// Заимствует дескриптор (например, чтобы продублировать), не отдавая владение.
    #[cfg(unix)]
    pub fn as_sync_fd(&self) -> Option<BorrowedFd<'_>> {
        self.state.as_sync_fd()
    }
}

/// Fence, который сигналит, когда GPU закончил работу с буфером (для `AImage_deleteAsync`).
///
/// Порождается после отправки работы на GPU. Владеет дескриптором так же, как [`GpuWaitFence`].
#[derive(Debug)]
pub struct GpuReleaseFence {
    state: FenceState,
}

impl GpuReleaseFence {
    /// Работа уже завершена: сигнальный fence без дескриптора.
    pub fn already_signaled() -> Self {
        Self {
            state: FenceState::AlreadySignaled,
        }
    }

    /// Берёт во владение настоящий `sync_fd` (его отдал драйвер при экспорте).
    #[cfg(unix)]
    pub fn from_sync_fd(fd: OwnedFd) -> Self {
        Self {
            state: FenceState::SyncFd(fd),
        }
    }

    /// Fence уже сигнален?
    pub fn is_already_signaled(&self) -> bool {
        self.state.is_already_signaled()
    }

    /// Отдаёт значение для NDK (`AImage_deleteAsync(image, releaseFenceFd)`): `-1`, если
    /// сигнален, иначе дескриптор.
    ///
    /// **Владение дескриптором переходит NDK**: после вызова NDK сам закроет fd, закрывать его
    /// повторно нельзя. Сам fence потребляется (`self`), поэтому двойное закрытие невозможно
    /// на уровне типов.
    pub fn into_raw_ndk_fd(self) -> i32 {
        self.state.into_raw_ndk()
    }

    /// Отдаёт дескриптор новому владельцу; `None` — fence был уже сигнален.
    #[cfg(unix)]
    pub fn into_owned_fd(self) -> Option<OwnedFd> {
        self.state.into_owned_fd()
    }

    /// Заимствует дескриптор (для ожидания на CPU через `gpu-vulkan`), не отдавая владение.
    #[cfg(unix)]
    pub fn as_sync_fd(&self) -> Option<BorrowedFd<'_>> {
        self.state.as_sync_fd()
    }
}

#[cfg(all(test, unix))]
#[path = "sync_fence_tests.rs"]
mod tests;
