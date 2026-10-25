//! JNI-экспорты `dev.fasticamera.NativeBridge` (Kotlin: `android/app/.../NativeBridge.kt`).
//!
//! Тонкий слой: строка из Java → вызов логики под `run_guarded` → JSON-строка в Java.
//! Имена символов зависят от `namespace` Android-сборки (`dev.fasticamera`), а не от
//! applicationId — поэтому debug-суффикс `.debug` их не ломает.

use jni::EnvUnowned;
use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{JClass, JString};

use crate::jni_boundary::run_guarded;
use crate::result_line::emit_result_line;
use crate::runtime::{InitReport, process_runtime};
use crate::scenario::execute;

/// Общая обвязка: строковый аргумент → `body` → строковый результат.
/// Ошибка JNI (например, не удалось прочитать строку) становится `RuntimeException`
/// в Kotlin — не молчаливым пустым результатом.
fn string_to_string<'local>(
    env: &mut EnvUnowned<'local>,
    input: &JString<'_>,
    body: impl FnOnce(String) -> String,
) -> JString<'local> {
    env.with_env(|env| -> jni::errors::Result<_> {
        let text = input.try_to_string(env)?;
        env.new_string(body(text))
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// `external fun initialize(configJson: String): String` — JSON `InitReport`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fasticamera_NativeBridge_initialize<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    config_json: JString<'local>,
) -> JString<'local> {
    string_to_string(&mut env, &config_json, |config_json| {
        let report =
            run_guarded(|| process_runtime().initialize(&config_json)).unwrap_or_else(|panic| InitReport::Failed {
                error: panic.log_text(),
            });
        // InitReport — enum из строк, сериализация не падает; запасной JSON — на невозможный сбой.
        serde_json::to_string(&report)
            .unwrap_or_else(|_| r#"{"status":"failed","error":"init_report_serialization"}"#.to_owned())
    })
}

/// `external fun runScenario(name: String): String` — JSON `ScenarioReport`; та же строка
/// уходит в logcat как `FASTICAMERA_RESULT {json}`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fasticamera_NativeBridge_runScenario<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    name: JString<'local>,
) -> JString<'local> {
    string_to_string(&mut env, &name, |name| {
        let report = execute(&name, process_runtime().config());
        emit_result_line(&report)
    })
}
