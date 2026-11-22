//! Safety-модуль сервисов снимка: два места, где память переходит границу Rust ↔ JVM.
//!
//! * [`with_direct_buffer`] — `NewDirectByteBuffer` поверх памяти Rust на время одного вызова;
//! * [`file_from_detached_fd`] — владение файловым дескриптором, отданным Kotlin (`detachFd`).

use std::fs::File;
use std::os::fd::{FromRawFd, RawFd};

use jni::Env;
use jni::objects::JByteBuffer;

/// Даёт `call` direct `ByteBuffer` поверх `bytes`. Буфер живёт ровно столько, сколько идёт `call`:
/// локальная ссылка наружу не выходит, а Kotlin-метод обязан не сохранять буфер (контракт
/// `StillJpegCompressor.compress`), иначе после возврата он указывал бы на чужую память.
pub(super) fn with_direct_buffer<'local, T>(
    env: &mut Env<'local>,
    bytes: &[u8],
    call: impl FnOnce(&mut Env<'local>, &JByteBuffer<'local>) -> jni::errors::Result<T>,
) -> jni::errors::Result<T> {
    // SAFETY: указатель и длина — ровно память `bytes`, живая всё время `call` (заём `bytes`
    // длится до конца функции). Память только читается: Kotlin копирует её в Bitmap и не пишет
    // (запись через указатель `*mut` на общую `&[u8]` — не выполняется ни Rust, ни Kotlin-кодом).
    // Буфер не переживает вызов: ссылка локальна для этой функции и в `call` передаётся по
    // заёму.
    let buffer = unsafe { env.new_direct_byte_buffer(bytes.as_ptr().cast_mut(), bytes.len()) }?;
    call(env, &buffer)
}

/// Файл поверх дескриптора из `ParcelFileDescriptor.detachFd()`.
///
/// Отрицательный дескриптор — ошибка платформы (`detachFd` возвращает -1 для закрытого
/// `ParcelFileDescriptor`), а не повод строить `File` из мусора.
pub(super) fn file_from_detached_fd(fd: RawFd) -> Result<File, String> {
    if fd < 0 {
        return Err(format!("detachFd вернул недействительный дескриптор {fd}"));
    }
    // SAFETY: `detachFd()` снимает владение с `ParcelFileDescriptor`: дескриптор открыт, и больше
    // никто его не закрывает. Мы получаем его один раз (значение берётся из одного вызова Kotlin и
    // сразу превращается в `File`), поэтому закрытие в `Drop` файла — единственное.
    Ok(unsafe { File::from_raw_fd(fd) })
}
