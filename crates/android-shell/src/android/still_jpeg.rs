//! Сжатие JPEG: `JpegCompressor` поверх Kotlin `StillJpegCompressor` (`Bitmap.compress`).
//!
//! Пиксели Rust передаются без копирования через direct `ByteBuffer` (см. `still_unsafe`),
//! результат — `byte[]` → `Vec<u8>`. Метод синхронный, зовётся с рабочего потока записи снимка.

use std::sync::Arc;

use jni::objects::{JByteArray, JValue};
use jni::{jni_sig, jni_str};
use shell_contract::still_media::{EncodeError, JpegCompressor};

use super::runtime::ShellRuntime;
use super::services::AndroidShellServices;
use super::still_jni::{call_platform, jpeg_compressor_object};
use super::still_unsafe::with_direct_buffer;
use crate::still_encode_input::prepare_jpeg_request;

/// JPEG-сжатие на телефоне. Создаётся один раз при сборке приложения (`new`).
pub struct AndroidJpegCompressor {
    runtime: Arc<ShellRuntime>,
}

impl AndroidJpegCompressor {
    pub fn new(services: &AndroidShellServices) -> Self {
        Self {
            runtime: Arc::clone(services.runtime()),
        }
    }
}

impl JpegCompressor for AndroidJpegCompressor {
    fn compress(&self, rgba: &[u8], width: u32, height: u32, quality: u8) -> Result<Vec<u8>, EncodeError> {
        let request = prepare_jpeg_request(rgba.len(), width, height, quality)?;
        let jpeg = call_platform(&self.runtime, |env, platform| {
            let compressor = jpeg_compressor_object(env, platform)?;
            let returned = with_direct_buffer(env, rgba, |env, pixels| {
                env.call_method(
                    &compressor,
                    jni_str!("compress"),
                    jni_sig!("(Ljava/nio/ByteBuffer;III)[B"),
                    &[
                        JValue::Object(pixels),
                        JValue::Int(request.width),
                        JValue::Int(request.height),
                        JValue::Int(request.quality),
                    ],
                )?
                .l()
            })?;
            let array = env.cast_local::<JByteArray>(returned)?;
            env.convert_byte_array(&array)
        })?;
        if jpeg.is_empty() {
            return Err(EncodeError::Platform("Bitmap.compress вернул пустой JPEG".to_owned()));
        }
        Ok(jpeg)
    }
}
