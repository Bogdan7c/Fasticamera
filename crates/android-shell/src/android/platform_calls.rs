//! Поток команд Rust → Kotlin (`dev.fasticamera.ShellPlatform`).
//!
//! Команды (вибрация, запрос разрешения, галерея, «не гасить экран») принимаются из любого
//! потока Rust без ожидания: они кладутся в ограниченную очередь, а выполняет их один поток,
//! навсегда подключённый к JVM. Так рендер и экран камеры никогда не ждут binder-вызовы
//! вибромотора (инвариант 4 README). Переполнение очереди — `Backpressure` со счётчиком.
//!
//! Kotlin-методы не принимают решений: вибромотор вызывается сразу, а операции с окном и
//! Activity Kotlin сам переносит на главный поток и при отказе сообщает
//! `ShellBridge.nativePlatformCommandFailed(код, причина)`.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread;

use jni::objects::{JObject, JValue};
use jni::{Env, JavaVM, jni_sig, jni_str};
use shell_contract::commands::{CommandOutcome, PlatformCommand, PlatformCommandFailure, ScreenWake};

use super::runtime::ShellRuntime;
use crate::haptic_plan::HapticRealization;

/// Ёмкость локального кадра JNI на одну команду (массивы волны + строки исключения).
const LOCAL_FRAME_CAPACITY: usize = 16;

/// Команда платформе.
#[derive(Debug, Clone, PartialEq)]
pub enum PlatformCall {
    RequestCameraPermission,
    PlayHaptic(HapticRealization),
    OpenSystemGallery,
    SetKeepScreenOn(ScreenWake),
}

impl PlatformCall {
    fn command(&self) -> PlatformCommand {
        match self {
            PlatformCall::RequestCameraPermission => PlatformCommand::RequestCameraPermission,
            PlatformCall::PlayHaptic(_) => PlatformCommand::PlayHaptic,
            PlatformCall::OpenSystemGallery => PlatformCommand::OpenSystemGallery,
            PlatformCall::SetKeepScreenOn(_) => PlatformCommand::KeepScreenOn,
        }
    }
}

/// Отправляющая сторона очереди команд.
#[derive(Debug, Clone)]
pub struct PlatformCallSender {
    channel: SyncSender<PlatformCall>,
}

impl PlatformCallSender {
    /// Ставит команду в очередь, не блокируясь.
    pub fn enqueue(&self, call: PlatformCall) -> CommandOutcome {
        match self.channel.try_send(call) {
            Ok(()) => CommandOutcome::Queued,
            Err(TrySendError::Full(call)) => {
                tracing::warn!(target: "fasticamera::shell", command = ?call.command(), "очередь команд полна, команда отброшена");
                CommandOutcome::Backpressure
            }
            // Поток команд не запустился или упал — выполнять некому.
            Err(TrySendError::Disconnected(_)) => CommandOutcome::NotAttached,
        }
    }
}

/// Создаёт очередь команд; поток запускается отдельно, когда готова оболочка.
pub fn platform_call_channel(capacity: usize) -> (PlatformCallSender, Receiver<PlatformCall>) {
    let (channel, receiver) = sync_channel(capacity);
    (PlatformCallSender { channel }, receiver)
}

/// Запускает поток команд (на весь процесс).
pub fn start_platform_call_thread(
    vm: JavaVM,
    runtime: Arc<ShellRuntime>,
    calls: Receiver<PlatformCall>,
) -> std::io::Result<()> {
    thread::Builder::new()
        .name("fasticamera-platform".to_owned())
        .spawn(move || {
            // attach_current_thread — постоянное подключение (отключится само при выходе потока).
            let outcome = vm.attach_current_thread(|env| -> jni::errors::Result<()> {
                for call in calls.iter() {
                    execute_call(env, &runtime, call);
                }
                Ok(())
            });
            if let Err(error) = outcome {
                tracing::error!(target: "fasticamera::shell", %error, "поток команд не подключился к JVM");
            }
        })
        .map(|_detached| ())
}

fn execute_call(env: &mut Env<'_>, runtime: &ShellRuntime, call: PlatformCall) {
    let command = call.command();
    let Some(platform) = runtime.attached_platform() else {
        // Activity ушла между постановкой и выполнением — выполнять некому, это не отказ платформы.
        tracing::debug!(target: "fasticamera::shell", ?command, "команда пропущена: Activity не подключена");
        return;
    };
    let result = env.with_local_frame(LOCAL_FRAME_CAPACITY, |env| -> jni::errors::Result<()> {
        invoke(env, platform.object(), &call)
    });
    if let Err(error) = result {
        let reason = describe_failure(env, &error);
        runtime.report_platform_failure(PlatformCommandFailure { command, reason });
    }
}

/// Вызов Kotlin-метода, соответствующего команде.
fn invoke(env: &mut Env<'_>, platform: &JObject<'_>, call: &PlatformCall) -> jni::errors::Result<()> {
    match call {
        PlatformCall::RequestCameraPermission => {
            env.call_method(platform, jni_str!("requestCameraPermission"), jni_sig!("()V"), &[])?;
        }
        PlatformCall::OpenSystemGallery => {
            env.call_method(platform, jni_str!("openSystemGallery"), jni_sig!("()V"), &[])?;
        }
        PlatformCall::SetKeepScreenOn(wake) => {
            let keep_on = matches!(wake, ScreenWake::KeepOn);
            env.call_method(
                platform,
                jni_str!("setKeepScreenOn"),
                jni_sig!("(Z)V"),
                &[JValue::Bool(keep_on)],
            )?;
        }
        PlatformCall::PlayHaptic(realization) => play_haptic(env, platform, realization)?,
    }
    Ok(())
}

fn play_haptic(env: &mut Env<'_>, platform: &JObject<'_>, realization: &HapticRealization) -> jni::errors::Result<()> {
    match realization {
        HapticRealization::Primitive { primitive, scale } => {
            env.call_method(
                platform,
                jni_str!("playPrimitive"),
                jni_sig!("(IF)V"),
                &[JValue::Int(primitive.android_code()), JValue::Float(*scale)],
            )?;
        }
        HapticRealization::PredefinedEffect(effect) => {
            env.call_method(
                platform,
                jni_str!("playPredefinedEffect"),
                jni_sig!("(I)V"),
                &[JValue::Int(effect.android_code())],
            )?;
        }
        HapticRealization::Waveform(steps) => {
            let timings: Vec<i64> = steps.iter().map(|step| i64::from(step.duration_ms)).collect();
            let amplitudes: Vec<i32> = steps.iter().map(|step| i32::from(step.amplitude)).collect();
            let timings_array = env.new_long_array(timings.len())?;
            timings_array.set_region(env, 0, &timings)?;
            let amplitudes_array = env.new_int_array(amplitudes.len())?;
            amplitudes_array.set_region(env, 0, &amplitudes)?;
            env.call_method(
                platform,
                jni_str!("playWaveform"),
                jni_sig!("([J[I)V"),
                &[JValue::Object(&timings_array), JValue::Object(&amplitudes_array)],
            )?;
        }
    }
    Ok(())
}

/// Текст отказа: для исключения Java — его описание (исключение при этом снимается),
/// иначе — текст ошибки JNI.
pub(super) fn describe_failure(env: &mut Env<'_>, error: &jni::errors::Error) -> String {
    if env.exception_check() {
        // exception_catch снимает исключение и превращает его в ошибку с текстом.
        return match env.exception_catch() {
            Err(caught) => caught.to_string(),
            Ok(()) => error.to_string(),
        };
    }
    error.to_string()
}
