//! JNI-экспорты `dev.fasticamera.probe.NativeBridge`. Тонкий слой: разбор аргументов,
//! вызов логики через `guarded_json`, упаковка результата. Контракт имён/сигнатур —
//! в `tools/device-probe/native/README` отчёта сессии и на стороне Kotlin.

use jni::EnvUnowned;
use jni::errors::LogErrorAndDefault;
use jni::objects::{JClass, JObject, JString};
use jni::sys::{jboolean, jint, jlong};

use crate::camera_ndk::build_catalog_json;
use crate::codec_surface::policy::RenderParams;
use crate::codec_surface::run_codec_surface_test_json;
use crate::codec_surface::window::NativeWindow;
use crate::error::ProbeError;
use crate::image_reader::{
    ReaderParams, create_reader_json, destroy_reader_json, reader_surface, run_import_test_json,
};
use crate::jni_boundary::guarded_json;
use crate::logging::ensure_initialized;
use crate::vulkan::info::build_vulkan_info_json;

/// Общая обвязка: выполняет `body` под защитой от паник и возвращает Java-строку с JSON.
fn json_to_java<'local>(
    env: &mut EnvUnowned<'local>,
    stage: &str,
    body: impl FnOnce() -> Result<String, ProbeError>,
) -> JString<'local> {
    ensure_initialized();
    env.with_env(|env| -> jni::errors::Result<_> {
        let json = guarded_json(stage, body);
        env.new_string(json)
    })
    .resolve::<LogErrorAndDefault>()
}

/// `external fun ndkCameraCatalog(): String`
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fasticamera_probe_NativeBridge_ndkCameraCatalog<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
) -> JString<'local> {
    json_to_java(&mut env, "ndkCameraCatalog", build_catalog_json)
}

/// `external fun vulkanInfo(): String`
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fasticamera_probe_NativeBridge_vulkanInfo<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
) -> JString<'local> {
    json_to_java(&mut env, "vulkanInfo", build_vulkan_info_json)
}

/// `external fun rawReaderCreate(width: Int, height: Int, format: Int, usage: Long, maxImages: Int): String`
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fasticamera_probe_NativeBridge_rawReaderCreate<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    width: jint,
    height: jint,
    format: jint,
    usage: jlong,
    max_images: jint,
) -> JString<'local> {
    // usage в Java знаковый Long; биты AHB-usage (в т.ч. 1<<32) переносим как есть.
    let params = ReaderParams {
        width,
        height,
        format,
        usage: usage as u64,
        max_images,
    };
    json_to_java(&mut env, "rawReaderCreate", move || create_reader_json(params))
}

/// `external fun rawReaderSurface(handle: Long): android.view.Surface?`
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fasticamera_probe_NativeBridge_rawReaderSurface<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    handle: jlong,
) -> JObject<'local> {
    ensure_initialized();
    env.with_env(|env| -> jni::errors::Result<_> {
        let raw_env = env.get_raw();
        // Паники здесь маловероятны (одни вызовы NDK), но граница FFI всё равно защищена `with_env`.
        // SAFETY: raw_env — валидный JNIEnv* текущего потока, выданный JVM для этого вызова.
        let surface = unsafe { reader_surface(handle, raw_env.cast()) };
        Ok(match surface {
            // SAFETY: surface — локальная JNI-ссылка, свежесозданная ANativeWindow_toSurface в этом кадре.
            Some(raw) => unsafe { JObject::from_raw(env, raw.cast()) },
            None => JObject::null(),
        })
    })
    .resolve::<LogErrorAndDefault>()
}

/// `external fun rawReaderRunImportTest(handle: Long, frames: Int, timeoutMs: Int): String`
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fasticamera_probe_NativeBridge_rawReaderRunImportTest<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    handle: jlong,
    frames: jint,
    timeout_ms: jint,
) -> JString<'local> {
    json_to_java(&mut env, "rawReaderRunImportTest", move || {
        run_import_test_json(handle, frames, timeout_ms)
    })
}

/// `external fun rawReaderDestroy(handle: Long): String`
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fasticamera_probe_NativeBridge_rawReaderDestroy<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    handle: jlong,
) -> JString<'local> {
    json_to_java(&mut env, "rawReaderDestroy", move || destroy_reader_json(handle))
}

/// `external fun codecSurfaceRenderTest(surface: android.view.Surface, width: Int, height: Int,
/// frames: Int, frameIntervalNs: Long, preferTenBit: Boolean): String`
///
/// Рендерит Vulkan'ом в входную Surface `MediaCodec`; всегда возвращает JSON по шагам.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fasticamera_probe_NativeBridge_codecSurfaceRenderTest<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    surface: JObject<'local>,
    width: jint,
    height: jint,
    frames: jint,
    frame_interval_ns: jlong,
    prefer_ten_bit: jboolean,
) -> JString<'local> {
    ensure_initialized();
    env.with_env(|env| -> jni::errors::Result<_> {
        // SAFETY: env — валидный JNIEnv* текущего вызова, surface — живая локальная ссылка на Surface.
        let window = unsafe { NativeWindow::from_surface(env.get_raw().cast(), surface.as_raw().cast()) };
        let params = RenderParams::from_jni(width, height, frames, frame_interval_ns, prefer_ten_bit);
        let json = guarded_json("codecSurfaceRenderTest", move || {
            run_codec_surface_test_json(window, params)
        });
        env.new_string(json)
    })
    .resolve::<LogErrorAndDefault>()
}
