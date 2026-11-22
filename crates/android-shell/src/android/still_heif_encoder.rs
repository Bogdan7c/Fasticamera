//! Кодер HEIF снимка: `HeifEncoderService` поверх Kotlin `StillHeifEncoder` (androidx `HeifWriter`).
//!
//! Порядок (решение 142): `open` создаёт кодер и отдаёт его входную поверхность как
//! `NativeSurface`; рендер рисует в неё ОДИН кадр; `finish` дописывает файл и закрывает кодер.
//! Rust держит глобальную ссылку на Kotlin-сессию до `finish`/`abort`; брошенный кодер
//! закрывается в `Drop` (лог), чтобы не оставлять открытый `MediaMuxer` и дескриптор.

use std::sync::Arc;
use std::time::Duration;

use jni::objects::{JObject, JValue};
use jni::{jni_sig, jni_str};
use shell_contract::still_media::{
    EncodeError, HeifEncodeSettings, HeifEncoderService, HeifSurfaceEncoder, PendingMedia,
};
use shell_contract::surface::{NativeSurface, SurfaceGeneration};

use super::native_window::NativeWindowRef;
use super::runtime::{CountedGlobal, ShellRuntime};
use super::services::AndroidShellServices;
use super::still_jni::{call_jvm, call_platform, heif_encoder_object};
use crate::still_encode_input::{heif_finish_result, prepare_heif_request, stop_timeout_millis};

/// Поколение поверхности кодера: нумерация поколений экрана начинается с 1, кодеру номер не нужен.
const ENCODER_SURFACE_GENERATION: SurfaceGeneration = SurfaceGeneration::new(0);

/// Фабрика кодеров HEIF на телефоне. Создаётся один раз при сборке приложения (`new`).
pub struct AndroidHeifEncoderService {
    runtime: Arc<ShellRuntime>,
}

impl AndroidHeifEncoderService {
    pub fn new(services: &AndroidShellServices) -> Self {
        Self {
            runtime: Arc::clone(services.runtime()),
        }
    }
}

impl HeifEncoderService for AndroidHeifEncoderService {
    fn open(
        &self,
        target: &dyn PendingMedia,
        settings: &HeifEncodeSettings,
    ) -> Result<Box<dyn HeifSurfaceEncoder>, EncodeError> {
        let request = prepare_heif_request(settings)?;
        let accounting = Arc::clone(self.runtime.accounting());
        let (session, window) = call_platform(&self.runtime, |env, platform| {
            let encoder = heif_encoder_object(env, platform)?;
            let uri = env.new_string(target.uri())?;
            // Без Exif — `null` (в Kotlin `ByteArray?`).
            let exif = match &request.exif_block {
                Some(block) => JObject::from(env.byte_array_from_slice(block)?),
                None => JObject::null(),
            };
            let session = env
                .call_method(
                    &encoder,
                    jni_str!("open"),
                    jni_sig!("(Ljava/lang/String;IIII[B)Ldev/fasticamera/StillHeifSession;"),
                    &[
                        JValue::Object(&uri),
                        JValue::Int(request.width),
                        JValue::Int(request.height),
                        JValue::Int(request.rotation_degrees),
                        JValue::Int(request.quality),
                        JValue::Object(&exif),
                    ],
                )?
                .l()?;
            // Кодер уже открыт: любой сбой дальше обязан его закрыть, иначе останутся открытый
            // `MediaMuxer`, поток кодера и дескриптор файла.
            let prepared = (|| -> jni::errors::Result<_> {
                // Глобальная ссылка — сессия переживёт этот локальный кадр.
                let global = CountedGlobal::new(env.new_global_ref(&session)?, Arc::clone(&accounting));
                let surface = env
                    .call_method(
                        &session,
                        jni_str!("inputSurface"),
                        jni_sig!("()Landroid/view/Surface;"),
                        &[],
                    )?
                    .l()?;
                Ok((global, NativeWindowRef::from_surface(env, &surface)))
            })();
            let (global, window) = match prepared {
                Ok(prepared) => prepared,
                Err(error) => {
                    // Висящее исключение Java снимаем с сохранением текста: вызывать `abort` с ним
                    // нельзя, а причина сбоя должна дойти до вызывающего.
                    let original = match env.exception_catch() {
                        Err(caught) => caught,
                        Ok(()) => error,
                    };
                    if let Err(abort_error) = env.call_method(&session, jni_str!("abort"), jni_sig!("()V"), &[]) {
                        env.exception_clear();
                        tracing::error!(target: "fasticamera::still", %abort_error, "кодер HEIF не закрылся после сбоя open");
                    }
                    return Err(original);
                }
            };
            Ok((global, window))
        })?;
        let mut encoder = AndroidHeifSurfaceEncoder {
            session: Some(session),
            input_surface: None,
        };
        match window {
            Some(window) => {
                encoder.input_surface = Some(NativeSurface::new(ENCODER_SURFACE_GENERATION, Box::new(window)))
            }
            None => {
                // Кодер уже открыт, а рисовать в него нельзя: закрываем (Drop) и сообщаем об отказе.
                return Err(EncodeError::Platform(
                    "у входной поверхности кодера нет окна (ANativeWindow_fromSurface = null)".to_owned(),
                ));
            }
        }
        Ok(Box::new(encoder))
    }
}

/// Открытый кодер одного снимка.
struct AndroidHeifSurfaceEncoder {
    /// Kotlin `StillHeifSession`. `None` — кодер уже закрыт (`finish`/`abort`).
    session: Option<CountedGlobal>,
    input_surface: Option<NativeSurface>,
}

impl HeifSurfaceEncoder for AndroidHeifSurfaceEncoder {
    fn take_input_surface(&mut self) -> Option<NativeSurface> {
        self.input_surface.take()
    }

    fn finish(mut self: Box<Self>, timeout: Duration) -> Result<(), EncodeError> {
        let Some(session) = self.session.take() else {
            return Err(EncodeError::Platform("кодер HEIF уже закрыт".to_owned()));
        };
        let timeout_millis = stop_timeout_millis(timeout);
        let code = call_jvm(|env| {
            env.call_method(
                session.object(),
                jni_str!("finish"),
                jni_sig!("(J)I"),
                &[JValue::Long(timeout_millis)],
            )?
            .i()
        })?;
        heif_finish_result(code, timeout)
    }

    fn abort(mut self: Box<Self>) {
        if let Some(session) = self.session.take() {
            abort_session(&session);
        }
    }
}

impl Drop for AndroidHeifSurfaceEncoder {
    fn drop(&mut self) {
        // `finish`/`abort` забирают сессию; она остаётся только если кодер выбросили без итога.
        if let Some(session) = self.session.take() {
            tracing::warn!(target: "fasticamera::still", "кодер HEIF выброшен без finish/abort: закрывается");
            abort_session(&session);
        }
    }
}

/// `StillHeifSession.abort()`; сбой — в лог (вернуть его некому).
fn abort_session(session: &CountedGlobal) {
    let result = call_jvm(|env| {
        env.call_method(session.object(), jni_str!("abort"), jni_sig!("()V"), &[])?;
        Ok(())
    });
    if let Err(error) = result {
        tracing::error!(target: "fasticamera::still", ?error, "кодер HEIF не закрылся при abort");
    }
}
