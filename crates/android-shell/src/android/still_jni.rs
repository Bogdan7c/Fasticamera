//! Общий вход для синхронных вызовов Kotlin-сервисов снимка с рабочего потока записи снимка.
//!
//! Поток не главный и не рендера (инвариант 4 README). Здесь:
//! * подключение потока к JVM (`attach_current_thread`: повторное подключение дёшево);
//! * локальный кадр JNI на вызов — локальные ссылки не копятся;
//! * висящее Java-исключение снимается и превращается в текст ошибки (`describe_failure`);
//! * Kotlin-объекты сервисов берутся у подключённой платформы (`ShellPlatform.still*()`);
//!   Activity нет — [`PlatformCallError::NotAttached`].

use jni::objects::JObject;
use jni::{Env, JavaVM, jni_sig, jni_str};

use super::platform_calls::describe_failure;
use super::runtime::ShellRuntime;
use crate::platform_call_error::PlatformCallError;

/// Ёмкость локального кадра JNI на один вызов (объекты сервиса, строки, массивы).
const LOCAL_FRAME_CAPACITY: usize = 16;

/// Выполняет `call` в подключённом потоке и локальном кадре. Платформа не нужна: так закрываются
/// и уже открытые кодеры, даже если Activity успела отключиться.
pub(super) fn call_jvm<T>(call: impl FnOnce(&mut Env<'_>) -> jni::errors::Result<T>) -> Result<T, PlatformCallError> {
    let vm = JavaVM::singleton().map_err(|error| PlatformCallError::Failed(format!("JavaVM не известна: {error}")))?;
    let outcome = vm.attach_current_thread(|env| -> jni::errors::Result<Result<T, String>> {
        let framed = env.with_local_frame(LOCAL_FRAME_CAPACITY, call);
        // Исключение снимаем здесь, пока `env` жив: после выхода из кадра оно осталось бы висеть.
        Ok(framed.map_err(|error| describe_failure(env, &error)))
    });
    match outcome {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(reason)) => Err(PlatformCallError::Failed(reason)),
        Err(error) => Err(PlatformCallError::Failed(format!(
            "поток не подключился к JVM: {error}"
        ))),
    }
}

/// То же, но с Kotlin-объектом платформы подключённой Activity.
pub(super) fn call_platform<T>(
    runtime: &ShellRuntime,
    call: impl FnOnce(&mut Env<'_>, &JObject<'_>) -> jni::errors::Result<T>,
) -> Result<T, PlatformCallError> {
    // Клон `Arc`: замок платформы не держится во время вызова в Java.
    let platform = runtime.attached_platform().ok_or(PlatformCallError::NotAttached)?;
    call_jvm(|env| call(env, platform.object()))
}

/// `ShellPlatform.stillMediaStore()`.
pub(super) fn media_store_object<'local>(
    env: &mut Env<'local>,
    platform: &JObject<'_>,
) -> jni::errors::Result<JObject<'local>> {
    env.call_method(
        platform,
        jni_str!("stillMediaStore"),
        jni_sig!("()Ldev/fasticamera/StillMediaStore;"),
        &[],
    )?
    .l()
}

/// `ShellPlatform.stillHeifEncoder()`.
pub(super) fn heif_encoder_object<'local>(
    env: &mut Env<'local>,
    platform: &JObject<'_>,
) -> jni::errors::Result<JObject<'local>> {
    env.call_method(
        platform,
        jni_str!("stillHeifEncoder"),
        jni_sig!("()Ldev/fasticamera/StillHeifEncoder;"),
        &[],
    )?
    .l()
}

/// `ShellPlatform.stillJpegCompressor()`.
pub(super) fn jpeg_compressor_object<'local>(
    env: &mut Env<'local>,
    platform: &JObject<'_>,
) -> jni::errors::Result<JObject<'local>> {
    env.call_method(
        platform,
        jni_str!("stillJpegCompressor"),
        jni_sig!("()Ldev/fasticamera/StillJpegCompressor;"),
        &[],
    )?
    .l()
}
