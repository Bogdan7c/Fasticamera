//! JNI-методы `dev.fasticamera.ShellBridge` (Kotlin: `android/app/.../ShellBridge.kt`).
//!
//! Регистрируются явно через `RegisterNatives` в `JNI_OnLoad` (без поиска символов по имени
//! в рантайме, требование сессии 02). Сигнатуры проверяет макрос `native_method!` на этапе
//! компиляции. Каждый колбэк — тонкий: перевести аргументы JNI в нейтральные типы и отправить
//! событие. Ошибки и паники не бросают исключений в Kotlin ([`LogAndCountCallbackFailure`]).

use jni::objects::{JClass, JIntArray, JObject, JString};
use jni::sys::{jint, jlong};
use jni::{Env, NativeMethod, jni_str, native_method};
use shell_contract::commands::PlatformCommandFailure;
use shell_contract::event::ShellEvent;
use shell_contract::surface::{NativeSurface, SurfaceReleaseOutcome, SurfaceSize, surface_release_handoff};

use super::callback_policy::LogAndCountCallbackFailure;
use super::haptic_query::query_haptic_capabilities;
use super::motion_event::copy_motion_event;
use super::native_window::NativeWindowRef;
use super::runtime::{AttachedPlatform, CountedGlobal, ShellRuntime, installed_runtime};
use crate::command_codes::platform_command_from_code;
use crate::key_decode::{DecodedKey, KeyConsumption, RawKeyEvent, decode_key_event};
use crate::permission_answer::{RationaleVisibility, classify_permission_answer};
use crate::safe_area_decode::decode_safe_area;
use crate::surface_registry::frame_rate_outcome;
use crate::system_gesture_decode::decode_system_gesture_zones;
use crate::text_input_wire::decode_text_input_result;
use crate::touch_decode::{DecodedMotion, decode_motion_event};

/// Класс Kotlin с `external`-методами оболочки.
const SHELL_BRIDGE_CLASS: &jni::strings::JNIStr = jni_str!("dev/fasticamera/ShellBridge");

const SHELL_NATIVE_METHODS: &[NativeMethod] = &[
    native_method! {
        static fn native_attach(platform: dev.fasticamera.ShellPlatform),
        error_policy = LogAndCountCallbackFailure,
    },
    native_method! {
        static fn native_detach(platform: dev.fasticamera.ShellPlatform),
        error_policy = LogAndCountCallbackFailure,
    },
    native_method! {
        static fn native_resumed(),
        error_policy = LogAndCountCallbackFailure,
    },
    native_method! {
        static fn native_paused(),
        error_policy = LogAndCountCallbackFailure,
    },
    native_method! {
        static fn native_surface_created(surface: android.view.Surface),
        error_policy = LogAndCountCallbackFailure,
    },
    native_method! {
        static fn native_surface_changed(width: jint, height: jint),
        error_policy = LogAndCountCallbackFailure,
    },
    native_method! {
        static fn native_surface_destroyed(),
        error_policy = LogAndCountCallbackFailure,
    },
    native_method! {
        static fn native_touch(motion_event: android.view.MotionEvent),
        error_policy = LogAndCountCallbackFailure,
    },
    native_method! {
        static fn native_hardware_key(key_code: jint, action: jint, repeat_count: jint, event_time_millis: jlong) -> bool,
        error_policy = LogAndCountCallbackFailure,
    },
    native_method! {
        static fn native_system_gesture_zones(left: jint, top: jint, right: jint, bottom: jint),
        error_policy = LogAndCountCallbackFailure,
    },
    native_method! {
        static fn native_safe_area(left: jint, top: jint, right: jint, bottom: jint),
        error_policy = LogAndCountCallbackFailure,
    },
    native_method! {
        static fn native_camera_permission_result(grant_results: jint[], should_show_rationale: bool),
        error_policy = LogAndCountCallbackFailure,
    },
    native_method! {
        static fn native_text_input_result(id: jlong, outcome_code: jint, text: JString),
        error_policy = LogAndCountCallbackFailure,
    },
    native_method! {
        static fn native_platform_command_failed(command_code: jint, reason: JString),
        error_policy = LogAndCountCallbackFailure,
    },
];

/// Регистрирует методы оболочки. Вызывается из `JNI_OnLoad` composition root.
pub fn register_natives(env: &mut Env<'_>) -> jni::errors::Result<()> {
    let class = env.find_class(SHELL_BRIDGE_CLASS)?;
    // SAFETY: каждый NativeMethod построен native_method!, который на этапе компиляции сверяет
    // сигнатуру Rust-функции с JNI-сигнатурой; класс — тот, для которого они объявлены.
    unsafe { env.register_native_methods(&class, SHELL_NATIVE_METHODS) }
}

/// Оболочка процесса или запись «колбэк до установки» (так быть не должно: Kotlin зовёт
/// `NativeBridge.initialize` раньше `ShellBridge.attach`).
fn runtime_or_warn(callback: &'static str) -> Option<&'static ShellRuntime> {
    let runtime = installed_runtime().map(|runtime| &**runtime);
    if runtime.is_none() {
        tracing::warn!(target: "fasticamera::shell", callback, "колбэк до установки оболочки пропущен");
    }
    runtime
}

/// Ошибка вне JNI (неверные данные платформы): лог и счётчик, исключения в Kotlin нет.
fn record_callback_failure(runtime: &ShellRuntime, callback: &'static str, error: &dyn std::error::Error) {
    runtime.accounting().record_native_callback_failure();
    tracing::error!(target: "fasticamera::shell", callback, %error, "колбэк оболочки не выполнен");
}

fn native_attach<'local>(
    env: &mut Env<'local>,
    _class: JClass<'local>,
    platform: JObject<'local>,
) -> jni::errors::Result<()> {
    let Some(runtime) = runtime_or_warn("attach") else {
        return Ok(());
    };
    // Осознанное исключение из «главный поток не ждёт»: ~10 коротких binder-вызовов к
    // вибромотору один раз за onCreate (замер сессии 02: вся подключённая Activity — 2–3 мс).
    // Взамен план хаптики знает возможности сразу и честно отвечает, каким способом сыграет.
    let capabilities = query_haptic_capabilities(env, &platform)?;
    tracing::info!(target: "fasticamera::shell", ?capabilities, "Activity подключена");
    let global = env.new_global_ref(&platform)?;
    let object = CountedGlobal::new(global, std::sync::Arc::clone(runtime.accounting()));
    runtime.attach_platform(AttachedPlatform::new(object, capabilities));
    Ok(())
}

fn native_detach<'local>(
    env: &mut Env<'local>,
    _class: JClass<'local>,
    platform: JObject<'local>,
) -> jni::errors::Result<()> {
    let Some(runtime) = runtime_or_warn("detach") else {
        return Ok(());
    };
    let detached = runtime.detach_platform_if(|attached| {
        // Ошибка сравнения (не должна случаться) — считаем «не та Activity», чтобы не отключить
        // новую, но не молча: прежняя Activity останется подключённой до следующего attach.
        env.is_same_object(attached.object(), &platform)
            .unwrap_or_else(|error| {
                tracing::error!(target: "fasticamera::shell", %error, "detach: объекты Activity не сравнились");
                false
            })
    });
    if !detached {
        tracing::debug!(target: "fasticamera::shell", "отключается не текущая Activity — пропуск");
    }
    Ok(())
}

fn native_resumed<'local>(_env: &mut Env<'local>, _class: JClass<'local>) -> jni::errors::Result<()> {
    if let Some(runtime) = runtime_or_warn("resumed") {
        runtime.send_event(ShellEvent::Resumed);
    }
    Ok(())
}

fn native_paused<'local>(_env: &mut Env<'local>, _class: JClass<'local>) -> jni::errors::Result<()> {
    if let Some(runtime) = runtime_or_warn("paused") {
        runtime.send_event(ShellEvent::Paused);
    }
    Ok(())
}

fn native_surface_created<'local>(
    env: &mut Env<'local>,
    _class: JClass<'local>,
    surface: JObject<'local>,
) -> jni::errors::Result<()> {
    let Some(runtime) = runtime_or_warn("surface_created") else {
        return Ok(());
    };
    let Some(window) = NativeWindowRef::from_surface(env, &surface) else {
        tracing::error!(target: "fasticamera::shell", "у новой Surface нет окна (ANativeWindow_fromSurface = null)");
        runtime.accounting().record_native_callback_failure();
        return Ok(());
    };
    // Реестру — своя ссылка (для частоты кадров), событию — своя (для рендера).
    // Замок реестра снимается в конце строки; частоту применяем уже без него.
    let (generation, pending_rate) = runtime.surfaces().surface_created(window.clone());
    let frame_rate = pending_rate.map(|rate| frame_rate_outcome(window.set_frame_rate(rate)));
    tracing::debug!(target: "fasticamera::shell", generation = generation.value(), ?frame_rate, "surface создана");
    runtime.send_event(ShellEvent::SurfaceCreated(NativeSurface::new(
        generation,
        Box::new(window),
    )));
    Ok(())
}

fn native_surface_changed<'local>(
    _env: &mut Env<'local>,
    _class: JClass<'local>,
    width: jint,
    height: jint,
) -> jni::errors::Result<()> {
    let Some(runtime) = runtime_or_warn("surface_changed") else {
        return Ok(());
    };
    let Some(generation) = runtime.surfaces().current_generation() else {
        tracing::warn!(target: "fasticamera::shell", "surfaceChanged без живой surface — пропуск");
        return Ok(());
    };
    let (Ok(width_px), Ok(height_px)) = (u32::try_from(width), u32::try_from(height)) else {
        tracing::error!(target: "fasticamera::shell", width, height, "отрицательный размер surface");
        runtime.accounting().record_native_callback_failure();
        return Ok(());
    };
    runtime.send_event(ShellEvent::SurfaceChanged {
        generation,
        size: SurfaceSize { width_px, height_px },
    });
    Ok(())
}

/// `surfaceDestroyed` — единственный колбэк, который ждёт: Android запрещает трогать surface
/// после возврата, поэтому ждём подтверждения владельца рендера (с таймаутом из конфига).
fn native_surface_destroyed<'local>(_env: &mut Env<'local>, _class: JClass<'local>) -> jni::errors::Result<()> {
    let Some(runtime) = runtime_or_warn("surface_destroyed") else {
        return Ok(());
    };
    let Some((generation, own_window)) = runtime.surfaces().surface_destroyed() else {
        tracing::warn!(target: "fasticamera::shell", "surfaceDestroyed без живой surface — пропуск");
        return Ok(());
    };
    let (request, waiter) = surface_release_handoff(generation);
    runtime.send_event(ShellEvent::SurfaceDestroyed(request));
    let timeout = runtime.config().surface_release_timeout;
    match waiter.wait(timeout) {
        SurfaceReleaseOutcome::Confirmed => {
            tracing::debug!(target: "fasticamera::shell", generation = generation.value(), "surface отпущена");
        }
        SurfaceReleaseOutcome::DroppedWithoutConfirmation => {
            tracing::warn!(
                target: "fasticamera::shell",
                generation = generation.value(),
                "квитанция surface уничтожена без подтверждения"
            );
        }
        SurfaceReleaseOutcome::TimedOut => {
            runtime.accounting().record_surface_release_timeout();
            tracing::error!(
                target: "fasticamera::shell",
                generation = generation.value(),
                timeout_ms = timeout.as_millis(),
                "владелец рендера не отпустил surface вовремя — возвращаем управление Android"
            );
        }
    }
    // Своя ссылка реестра уходит последней — после того, как рендер отпустил свою.
    drop(own_window);
    Ok(())
}

fn native_touch<'local>(
    env: &mut Env<'local>,
    _class: JClass<'local>,
    motion_event: JObject<'local>,
) -> jni::errors::Result<()> {
    let Some(runtime) = runtime_or_warn("touch") else {
        return Ok(());
    };
    let raw = match copy_motion_event(env, &motion_event) {
        Ok(raw) => raw,
        Err(error) => {
            record_callback_failure(runtime, "touch", &error);
            return Ok(());
        }
    };
    match decode_motion_event(&raw) {
        Ok(DecodedMotion::Samples(samples)) => {
            for sample in samples {
                // Переполнение считает очередь (и добавит TouchGap); главный поток не ждёт.
                runtime.send_event(ShellEvent::Touch(sample));
            }
        }
        Ok(DecodedMotion::Ignored { masked_action }) => {
            tracing::trace!(target: "fasticamera::shell", masked_action, "действие касания пропущено");
        }
        Err(error) => record_callback_failure(runtime, "touch", &error),
    }
    Ok(())
}

/// Аппаратная клавиша. Возвращает `true`, если клавиша наша и Kotlin должен потребить событие
/// (громкость не меняется, ползунок не всплывает). Оболочка не установлена или событие
/// некорректно — `false`: система обработает клавишу как обычно, а не «проглотит» её молча.
fn native_hardware_key<'local>(
    _env: &mut Env<'local>,
    _class: JClass<'local>,
    key_code: jint,
    action: jint,
    repeat_count: jint,
    event_time_millis: jlong,
) -> jni::errors::Result<bool> {
    let Some(runtime) = runtime_or_warn("hardware_key") else {
        return Ok(false);
    };
    let raw = RawKeyEvent {
        key_code,
        action,
        repeat_count,
        event_time_ms: event_time_millis,
    };
    match decode_key_event(&raw) {
        Ok(DecodedKey::Handled(event)) => {
            // Очередь не блокирует; потерю критичного события считает и логирует send_event.
            // Потребляем клавишу только если приложение её действительно получило.
            let outcome = runtime.send_event(ShellEvent::HardwareKey(event));
            Ok(KeyConsumption::after_send(outcome).is_consumed())
        }
        Ok(DecodedKey::NotOurKey { key_code }) => {
            tracing::trace!(target: "fasticamera::shell", key_code, "клавиша не наша — оставлена системе");
            Ok(false)
        }
        Err(error) => {
            record_callback_failure(runtime, "hardware_key", &error);
            Ok(false)
        }
    }
}

/// Зоны обязательных системных жестов (`WindowInsets.Type.mandatorySystemGestures()`), пиксели окна в порядке
/// `Insets`: left, top, right, bottom. Событие-состояние: отправка без блокировки; потерю при полной очереди
/// считает и логирует `send_event`. Невозможные числа — лог и счётчик, прежние зоны остаются.
fn native_system_gesture_zones<'local>(
    _env: &mut Env<'local>,
    _class: JClass<'local>,
    left: jint,
    top: jint,
    right: jint,
    bottom: jint,
) -> jni::errors::Result<()> {
    let Some(runtime) = runtime_or_warn("system_gesture_zones") else {
        return Ok(());
    };
    match decode_system_gesture_zones(left, top, right, bottom) {
        Ok(zones) => {
            runtime.send_event(ShellEvent::SystemGestureZones(zones));
        }
        Err(error) => record_callback_failure(runtime, "system_gesture_zones", &error),
    }
    Ok(())
}

/// Безопасная зона (объединение системных панелей, выреза и полос обязательных жестов), пиксели окна в порядке
/// `Insets`: left, top, right, bottom. Событие-состояние; невозможные числа — лог и счётчик, прежняя зона остаётся.
fn native_safe_area<'local>(
    _env: &mut Env<'local>,
    _class: JClass<'local>,
    left: jint,
    top: jint,
    right: jint,
    bottom: jint,
) -> jni::errors::Result<()> {
    let Some(runtime) = runtime_or_warn("safe_area") else {
        return Ok(());
    };
    match decode_safe_area(left, top, right, bottom) {
        Ok(insets) => {
            runtime.send_event(ShellEvent::SafeArea(insets));
        }
        Err(error) => record_callback_failure(runtime, "safe_area", &error),
    }
    Ok(())
}

fn native_camera_permission_result<'local>(
    env: &mut Env<'local>,
    _class: JClass<'local>,
    grant_results: JIntArray<'local>,
    should_show_rationale: bool,
) -> jni::errors::Result<()> {
    let Some(runtime) = runtime_or_warn("camera_permission_result") else {
        return Ok(());
    };
    let length = grant_results.len(env)?;
    let mut results = vec![0; length];
    grant_results.get_region(env, 0, &mut results)?;
    let permission = classify_permission_answer(
        &results,
        RationaleVisibility::from_should_show_rationale(should_show_rationale),
    );
    tracing::info!(target: "fasticamera::shell", permission = permission.name(), "ответ на запрос разрешения камеры");
    runtime.send_event(ShellEvent::CameraPermission(permission));
    Ok(())
}

fn native_platform_command_failed<'local>(
    env: &mut Env<'local>,
    _class: JClass<'local>,
    command_code: jint,
    reason: JString<'local>,
) -> jni::errors::Result<()> {
    let Some(runtime) = runtime_or_warn("platform_command_failed") else {
        return Ok(());
    };
    let reason = reason.try_to_string(env)?;
    match platform_command_from_code(command_code) {
        Some(command) => runtime.report_platform_failure(PlatformCommandFailure { command, reason }),
        None => {
            runtime.accounting().record_native_callback_failure();
            tracing::error!(
                target: "fasticamera::shell",
                command_code,
                %reason,
                "Kotlin сообщил об отказе неизвестной команды (контракт ShellPlatform.COMMAND_*)"
            );
        }
    }
    Ok(())
}

/// Ответ диалога ввода текста: `id` — номер запроса, `outcome_code` — `TextInputDialog.OUTCOME_*`, `text` — строка
/// (только для «введено»; для остальных итогов Kotlin передаёт `null`). Главный поток не ждёт: событие уходит
/// в очередь без блокировки. Нарушение контракта (неизвестный код, «введено» без текста) — лог и счётчик;
/// запрос при этом остаётся без ответа до следующего запроса (его вытеснит новый).
fn native_text_input_result<'local>(
    env: &mut Env<'local>,
    _class: JClass<'local>,
    id: jlong,
    outcome_code: jint,
    text: JString<'local>,
) -> jni::errors::Result<()> {
    let Some(runtime) = runtime_or_warn("text_input_result") else {
        return Ok(());
    };
    // null — «текста нет»: допустим для отмены и «недоступно», ошибка для «введено» (решает декодер).
    let text = if text.is_null() {
        None
    } else {
        Some(text.try_to_string(env)?)
    };
    match decode_text_input_result(id, outcome_code, text) {
        Ok(result) => {
            tracing::info!(target: "fasticamera::shell", id = result.id.0, "ответ диалога ввода текста");
            runtime.send_event(ShellEvent::TextInput(result));
        }
        Err(error) => record_callback_failure(runtime, "text_input_result", &error),
    }
    Ok(())
}
