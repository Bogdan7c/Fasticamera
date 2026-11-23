//! Вход JNI библиотеки: `JNI_OnLoad` и методы `dev.fasticamera.NativeBridge`
//! (Kotlin: `android/app/.../NativeBridge.kt`).
//!
//! Все нативные методы регистрируются явно через `RegisterNatives` в `JNI_OnLoad`, без поиска
//! символов `Java_*` по имени в рантайме (требование сессии 02): и методы `NativeBridge`
//! (здесь), и методы оболочки `ShellBridge` (`android_shell::android::register_natives`).
//! Сигнатуры сверяет макрос `native_method!` при компиляции; ошибка регистрации — громкая:
//! `JNI_OnLoad` вернёт `JNI_ERR`, и `System.loadLibrary` бросит исключение.

use std::ffi::c_void;

use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{JClass, JString};
use jni::sys::{JNI_ERR, JNI_VERSION_1_6, jint};
use jni::{Env, JavaVM, NativeMethod, jni_str, native_method};

use android_shell::package_name::PackageName;

use crate::app_directory::AppDirectory;

use crate::jni_boundary::run_guarded;
use crate::logging::{LogPriority, emit_unfiltered};
use crate::result_line::emit_result_line;
use crate::runtime::{InitReport, process_runtime};
use crate::scenario::{DispatchOutcome, dispatch};
use crate::shell_wiring;

const NATIVE_BRIDGE_CLASS: &jni::strings::JNIStr = jni_str!("dev/fasticamera/NativeBridge");

const NATIVE_BRIDGE_METHODS: &[NativeMethod] = &[
    native_method! {
        static fn initialize(config_json: JString, package_name: JString, app_directory: JString) -> JString,
        error_policy = ThrowRuntimeExAndDefault,
    },
    native_method! {
        static fn run_scenario(name: JString) -> JString,
        error_policy = ThrowRuntimeExAndDefault,
    },
];

/// Точка входа JVM при `System.loadLibrary("fasticamera")`.
#[unsafe(no_mangle)]
pub extern "system" fn JNI_OnLoad(vm: *mut jni::sys::JavaVM, _reserved: *mut c_void) -> jint {
    // SAFETY: указатель JavaVM передаёт сама JVM; он действителен всё время жизни процесса.
    // from_raw запоминает JavaVM как процессный singleton (нужен потокам оболочки).
    let vm = unsafe { JavaVM::from_raw(vm) };
    let registered = run_guarded(|| vm.attach_current_thread(register_all_natives));
    match registered {
        Ok(Ok(())) => JNI_VERSION_1_6,
        Ok(Err(error)) => {
            // Логирование ещё не установлено (оно ставится в initialize) — пишем мимо фильтра.
            emit_unfiltered(
                LogPriority::Fatal,
                format!("JNI_OnLoad: регистрация нативных методов не удалась: {error}"),
            );
            JNI_ERR
        }
        Err(panic) => {
            emit_unfiltered(LogPriority::Fatal, format!("JNI_OnLoad: паника: {}", panic.log_text()));
            JNI_ERR
        }
    }
}

fn register_all_natives(env: &mut Env<'_>) -> jni::errors::Result<()> {
    let class = env.find_class(NATIVE_BRIDGE_CLASS)?;
    // SAFETY: методы построены native_method!, сигнатуры сверены при компиляции с Rust-функциями
    // ниже; класс — тот, для которого они объявлены.
    unsafe { env.register_native_methods(&class, NATIVE_BRIDGE_METHODS)? };
    android_shell::android::register_natives(env)
}

/// `external fun initialize(configJson: String, packageName: String, appDirectory: String): String` — JSON
/// `InitReport`. Принимает конфиг, имя пакета процесса и папку приложения, ставит логирование, затем оболочку и
/// маршрутизатор событий. Имя пакета нужно оболочке для датчиков с первого кадра (решение 106), поэтому приходит
/// здесь, а не при подключении Activity. Папка приложения (`filesDir`) нужна хранилищу пресетов вида (решение 99)
/// ДО старта рендера: начальный вид действует с первого кадра.
fn initialize<'local>(
    env: &mut Env<'local>,
    _class: JClass<'local>,
    config_json: JString<'local>,
    package_name: JString<'local>,
    app_directory: JString<'local>,
) -> jni::errors::Result<JString<'local>> {
    let config_json = config_json.try_to_string(env)?;
    let package_name = package_name.try_to_string(env)?;
    let app_directory = app_directory.try_to_string(env)?;
    let report = run_guarded(|| initialize_runtime_and_shell(&config_json, package_name, app_directory))
        .unwrap_or_else(|panic| InitReport::Failed {
            error: panic.log_text(),
        });
    // InitReport — enum из строк, сериализация не падает; запасной JSON — на невозможный сбой.
    let json = serde_json::to_string(&report)
        .unwrap_or_else(|_| r#"{"status":"failed","error":"init_report_serialization"}"#.to_owned());
    env.new_string(json)
}

fn initialize_runtime_and_shell(config_json: &str, package_name: String, app_directory: String) -> InitReport {
    let report = process_runtime().initialize(config_json);
    if matches!(report, InitReport::Failed { .. }) {
        return report;
    }
    let Some(config) = process_runtime().config() else {
        return InitReport::Failed {
            error: "конфиг не сохранён после успешного initialize".to_owned(),
        };
    };
    let package_name = match PackageName::new(package_name) {
        Ok(name) => name,
        Err(error) => {
            return InitReport::Failed {
                error: format!("имя пакета не годится: {error}"),
            };
        }
    };
    // Каталог проверяется различимой ошибкой, а не первой неудачной записью пресетов: его нет — запуск отвергнут.
    let app_directory = match AppDirectory::new(app_directory) {
        Ok(directory) => directory,
        Err(error) => {
            return InitReport::Failed {
                error: format!("папка приложения не годится: {error}"),
            };
        }
    };
    // Повторный initialize (новая Activity в том же процессе) не ставит оболочку второй раз.
    match shell_wiring::ensure_installed(config, package_name, &app_directory) {
        Ok(()) => report,
        Err(error) => InitReport::Failed {
            error: format!("оболочка не установлена: {error}"),
        },
    }
}

/// `external fun runScenario(name: String): String` — JSON итога (мгновенный сценарий) или
/// `{"status":"started"}` (наблюдающий: итог придёт строкой `FASTICAMERA_RESULT` позже).
fn run_scenario<'local>(
    env: &mut Env<'local>,
    _class: JClass<'local>,
    name: JString<'local>,
) -> jni::errors::Result<JString<'local>> {
    let name = name.try_to_string(env)?;
    let json = match dispatch(&name, process_runtime().config(), shell_wiring::start_observing) {
        DispatchOutcome::Finished(report) => emit_result_line(&report),
        DispatchOutcome::Started { scenario } => {
            serde_json::json!({"scenario": scenario, "status": "started"}).to_string()
        }
    };
    env.new_string(json)
}
