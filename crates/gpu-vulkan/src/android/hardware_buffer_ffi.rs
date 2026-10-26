//! Сырые объявления функций NDK `AHardwareBuffer` (решение 32 сессии 03, без `ndk-sys`).
//!
//! Это ЕДИНСТВЕННЫЙ файл crate с `extern "C"` и `#[link]` — так требует guardrail
//! `gpu-ffi-only-in-safety-module` (`scripts/crate-boundaries.toml`). Здесь только объявления;
//! безопасные обёртки и их `// SAFETY:` — в [`super::hardware_buffer`].
//!
//! Сигнатуры сверены с заголовком NDK 30
//! (`toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/include/android/hardware_buffer.h`);
//! в скобках — номера строк этого файла:
//! * `AHardwareBuffer_Desc` (422–437);
//! * `AHardwareBuffer_allocate` (479, API 26), `_acquire` (489, API 26), `_release` (497, API 26),
//!   `_describe` (505, API 26), `_lock` (560–562, API 26), `_unlock` (582–583, API 26);
//! * `AHardwareBuffer_getId` (686–687, API 31; minSdk приложения 36);
//! * `ARect` — `android/rect.h:45` (четыре `int32_t`).
//!
//! Библиотека `nativewindow` входит в NDK (`libnativewindow.so`) и есть в системе с API 26.

use std::ffi::{c_int, c_void};

pub use ash::vk::AHardwareBuffer;

/// `AHardwareBuffer_Desc` (hardware_buffer.h:422). Поле `stride` — «в пикселях» для
/// обычных форматов; для упакованного RAW10 телефона это БАЙТЫ строки (факт 00, дефект 5a).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AHardwareBufferDesc {
    /// Ширина, пиксели.
    pub width: u32,
    /// Высота, пиксели.
    pub height: u32,
    /// Число слоёв (для обычных 2D-буферов — 1).
    pub layers: u32,
    /// Формат (`AHARDWAREBUFFER_FORMAT_*` или код HAL, например RAW16 = 0x20).
    pub format: u32,
    /// Комбинация `AHARDWAREBUFFER_USAGE_*`.
    pub usage: u64,
    /// Шаг строки (см. выше про единицы). При `allocate` игнорируется.
    pub stride: u32,
    /// Зарезервировано, должно быть 0.
    pub rfu0: u32,
    /// Зарезервировано, должно быть 0.
    pub rfu1: u64,
}

/// `ARect` (android/rect.h:45): прямоугольник блокировки.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ARect {
    /// Левая граница.
    pub left: i32,
    /// Верхняя граница.
    pub top: i32,
    /// Правая граница (не включительно).
    pub right: i32,
    /// Нижняя граница (не включительно).
    pub bottom: i32,
}

/// `AHARDWAREBUFFER_USAGE_CPU_READ_OFTEN` (hardware_buffer.h:295).
pub const USAGE_CPU_READ_OFTEN: u64 = 3;
/// `AHARDWAREBUFFER_USAGE_CPU_WRITE_OFTEN` (hardware_buffer.h:319) = `3 << 4`.
pub const USAGE_CPU_WRITE_OFTEN: u64 = 3 << 4;
/// `AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE` (hardware_buffer.h:323) = `1 << 8`.
pub const USAGE_GPU_SAMPLED_IMAGE: u64 = 1 << 8;
/// `AHARDWAREBUFFER_USAGE_GPU_DATA_BUFFER` (hardware_buffer.h:370) = `1 << 24`.
pub const USAGE_GPU_DATA_BUFFER: u64 = 1 << 24;

#[link(name = "nativewindow")]
unsafe extern "C" {
    /// Выделяет буфер (счётчик ссылок = 1). `0` — успех, иначе код ошибки (hardware_buffer.h:479).
    pub fn AHardwareBuffer_allocate(desc: *const AHardwareBufferDesc, out_buffer: *mut *mut AHardwareBuffer) -> c_int;

    /// Увеличивает счётчик ссылок (hardware_buffer.h:489).
    pub fn AHardwareBuffer_acquire(buffer: *mut AHardwareBuffer);

    /// Уменьшает счётчик ссылок; на нуле буфер освобождается (hardware_buffer.h:497).
    pub fn AHardwareBuffer_release(buffer: *mut AHardwareBuffer);

    /// Заполняет описание буфера (hardware_buffer.h:505).
    pub fn AHardwareBuffer_describe(buffer: *const AHardwareBuffer, out_desc: *mut AHardwareBufferDesc);

    /// Блокирует буфер для доступа CPU (hardware_buffer.h:560). `fence` — fd ожидания или `-1`;
    /// `rect` — `NULL` для всего буфера. `0` — успех.
    pub fn AHardwareBuffer_lock(
        buffer: *mut AHardwareBuffer,
        usage: u64,
        fence: i32,
        rect: *const ARect,
        out_virtual_address: *mut *mut c_void,
    ) -> c_int;

    /// Разблокирует буфер (hardware_buffer.h:582). `fence == NULL` — блокирующий вызов до
    /// завершения; иначе в `*fence` вернётся fd (его закрывает вызывающий) или `-1`. `0` — успех.
    pub fn AHardwareBuffer_unlock(buffer: *mut AHardwareBuffer, fence: *mut i32) -> c_int;

    /// Системный идентификатор буфера (hardware_buffer.h:686, API 31). `0` — успех.
    pub fn AHardwareBuffer_getId(buffer: *const AHardwareBuffer, out_id: *mut u64) -> c_int;
}
