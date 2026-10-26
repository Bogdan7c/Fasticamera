//! Политика ошибок JNI-колбэков оболочки (`jni::errors::ErrorPolicy`).
//!
//! Колбэки жизненного цикла, surface и касаний вызывает Android. Исключение, брошенное
//! в такой колбэк, уронило бы приложение. Поэтому:
//! * ошибка или паника колбэка не превращается в исключение Java;
//! * исключение, которое бросил вызванный из колбэка Java-метод, снимается;
//! * всё это пишется в лог (паника, кроме того, уже записана хуком паник с местом) и
//!   учитывается в счётчике `native_callback_failures`; Kotlin получает значение по умолчанию.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};

use jni::Env;
use jni::errors::ErrorPolicy;

use super::runtime::installed_runtime;

/// Пишет в лог и считает, исключение в Java не бросает.
#[derive(Debug, Default)]
pub struct LogAndCountCallbackFailure;

impl<T: Default, E: std::error::Error> ErrorPolicy<T, E> for LogAndCountCallbackFailure {
    type Captures<'unowned_env_local: 'native_method, 'native_method> = ();

    fn on_error<'unowned_env_local: 'native_method, 'native_method>(
        env: &mut Env<'unowned_env_local>,
        _captures: &mut Self::Captures<'unowned_env_local, 'native_method>,
        error: E,
    ) -> jni::errors::Result<T> {
        let java_exception = take_pending_java_exception(env);
        tracing::error!(
            target: "fasticamera::shell",
            %error,
            java_exception = java_exception.as_deref().unwrap_or("нет"),
            "колбэк оболочки завершился ошибкой JNI"
        );
        count_failure();
        Ok(T::default())
    }

    fn on_panic<'unowned_env_local: 'native_method, 'native_method>(
        env: &mut Env<'unowned_env_local>,
        _captures: &mut Self::Captures<'unowned_env_local, 'native_method>,
        payload: Box<dyn Any + Send + 'static>,
    ) -> jni::errors::Result<T> {
        // Паника могла случиться посреди вызова в Java, который уже бросил исключение.
        let java_exception = take_pending_java_exception(env);
        tracing::error!(
            target: "fasticamera::shell",
            java_exception = java_exception.as_deref().unwrap_or("нет"),
            "колбэк оболочки запаниковал (место — в записи FATAL хука паник)"
        );
        count_failure();
        // Уничтожение полезной нагрузки паники само может запаниковать — не выпускаем это в JVM.
        if catch_unwind(AssertUnwindSafe(|| drop(payload))).is_err() {
            tracing::error!(target: "fasticamera::shell", "паника при уничтожении полезной нагрузки паники");
        }
        Ok(T::default())
    }
}

/// Снимает исключение Java, оставшееся от неудачного вызова в Java, и возвращает его текст.
///
/// Без этого исключение так и висело бы в JNIEnv и вылетело бы в Kotlin при возврате из
/// нативного метода, хотя политика обещает «без исключений» (`ThrowRuntimeExAndDefault`
/// тоже сначала проверяет `exception_check`, но оставляет исключение — нам нужно обратное).
fn take_pending_java_exception(env: &mut Env<'_>) -> Option<String> {
    if !env.exception_check() {
        return None;
    }
    // exception_catch снимает исключение и превращает его в ошибку с описанием.
    match env.exception_catch() {
        Err(caught) => Some(caught.to_string()),
        Ok(()) => Some("исключение снято без описания".to_owned()),
    }
}

fn count_failure() {
    if let Some(runtime) = installed_runtime() {
        runtime.accounting().record_native_callback_failure();
    }
}
