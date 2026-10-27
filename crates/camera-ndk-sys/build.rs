//! Сборка биндингов Camera2 NDK: bindgen по заголовкам закреплённого NDK (решение 37).
//!
//! * Не Android — ничего не делаем: crate на хосте пустой.
//! * Android — NDK и libclang ищет чистая логика из `build_support/ndk_toolchain.rs`
//!   (она же тестируется в `cargo test`): `FASTICAMERA_LIBCLANG_PATH` → `ANDROID_NDK_HOME` →
//!   `<SDK>/ndk/<версия из gradle.properties>`. Нет NDK или libclang → сборка падает с текстом
//!   «что проверено и что сделать», молчаливого пропуска нет.
//! * Линковка `camera2ndk` и `mediandk` объявляется здесь и только под Android.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[path = "build_support/ndk_toolchain.rs"]
mod ndk_toolchain;

use ndk_toolchain::{
    NDK_VERSION_PROPERTY, NdkToolchain, PLATFORM_API_PROPERTY, ToolchainEnvironment, clang_arguments, clang_target_for,
    parse_gradle_property, required_platform_api, resolve_toolchain,
};

/// Переменные окружения, от которых зависит результат сборки.
const WATCHED_VARIABLES: [&str; 6] = [
    "FASTICAMERA_LIBCLANG_PATH",
    "ANDROID_NDK_HOME",
    "ANDROID_HOME",
    "ANDROID_SDK_ROOT",
    "HOME",
    "CARGO_CFG_TARGET_ARCH",
];

/// Разрешённые функции: только то, что нужно бэкенду камеры (по префиксам имён NDK).
const ALLOWED_FUNCTIONS: [&str; 13] = [
    "ACameraManager_.*",
    "ACameraDevice_.*",
    "ACameraCaptureSession_.*",
    "ACaptureRequest_.*",
    "ACaptureSessionOutput.*",
    "ACameraOutputTarget_.*",
    // Не `ACameraMetadata_.*`: `ACameraMetadata_fromCameraMetadata` принимает JNIEnv и тянет за собой весь JNI
    // (blocklist этого не отменяет), а JNI камере запрещён guardrail-ом jni-ndk-only-in-shell.
    "ACameraMetadata_(getConstEntry|getAllTags|getTagFromName|copy|free|isLogicalMultiCamera)",
    "AImageReader_.*",
    "AImage_.*",
    // Слияние fence-ов освобождения кадра (libsync, `<android/sync.h>`) и доступ CPU к буферу для
    // отладочной пробы «0 копий» (`AHardwareBuffer_lock/unlock/describe`).
    "sync_merge",
    "AHardwareBuffer_lock",
    "AHardwareBuffer_unlock",
    "AHardwareBuffer_describe",
];

/// Разрешённые типы (структуры колбэков, непрозрачные хэндлы, коды статусов).
const ALLOWED_TYPES: [&str; 14] = [
    "ACamera.*",
    "ACapture.*",
    "ALogicalCamera.*",
    "AImage.*",
    "camera_status_t",
    "media_status_t",
    "acamera_metadata_.*",
    "AIMAGE_FORMATS",
    "ANativeWindow",
    "AHardwareBuffer",
    "AHardwareBuffer_UsageFlags",
    "AHardwareBuffer_Desc",
    "ARect",
    "ACameraWindowType",
];

/// Разрешённые константы: теги и значения `ACAMERA_*`, форматы, usage-флаги, коды ошибок.
const ALLOWED_CONSTANTS: [&str; 9] = [
    "ACAMERA_.*",
    "AIMAGE_FORMAT_.*",
    "AHARDWAREBUFFER_USAGE_.*",
    "AMEDIA_.*",
    "TEMPLATE_.*",
    "ERROR_CAMERA_.*",
    "CAPTURE_FAILURE_REASON_.*",
    "CAPTURE_SEQUENCE_ID_NONE",
    "AMEDIA_IMGREADER_.*",
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build_support");
    println!("cargo:rerun-if-changed=wrapper.h");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("android") {
        return;
    }
    if let Err(message) = generate_bindings() {
        eprintln!("\nошибка сборки camera-ndk-sys: {message}\n");
        std::process::exit(1);
    }
}

/// Читает окружение процесса в структуру для чистой логики поиска.
fn read_environment(gradle_properties: &Path) -> ToolchainEnvironment {
    let path_variable = |name: &str| env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from);
    let mut sdk_roots: Vec<PathBuf> = ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
        .iter()
        .filter_map(|name| path_variable(name))
        .collect();
    if let Some(home) = path_variable("HOME") {
        sdk_roots.push(home.join("Android").join("Sdk"));
    }
    let properties = fs::read_to_string(gradle_properties).ok();
    let property = |key: &str| properties.as_deref().and_then(|text| parse_gradle_property(text, key));
    ToolchainEnvironment {
        libclang_override: path_variable("FASTICAMERA_LIBCLANG_PATH"),
        ndk_home: path_variable("ANDROID_NDK_HOME"),
        sdk_roots,
        ndk_version: property(NDK_VERSION_PROPERTY),
        platform_api: property(PLATFORM_API_PROPERTY),
        host_os: env::consts::OS.to_owned(),
    }
}

fn generate_bindings() -> Result<(), String> {
    let manifest_directory = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("нет CARGO_MANIFEST_DIR")?);
    let out_directory = PathBuf::from(env::var_os("OUT_DIR").ok_or("нет OUT_DIR")?);
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").map_err(|_| "нет CARGO_CFG_TARGET_ARCH".to_owned())?;
    let gradle_properties = manifest_directory.join("../../android/gradle.properties");
    println!("cargo:rerun-if-changed={}", gradle_properties.display());
    for variable in WATCHED_VARIABLES {
        println!("cargo:rerun-if-env-changed={variable}");
    }

    let environment = read_environment(&gradle_properties);
    let platform_api = required_platform_api(&environment).map_err(|error| error.to_string())?;
    let toolchain = resolve_toolchain(&environment, &|path: &Path| path.exists()).map_err(|error| error.to_string())?;
    if !toolchain.matches_pinned_version {
        println!(
            "cargo:warning=camera-ndk-sys собирается по NDK {}, а не по закреплённому {NDK_VERSION_PROPERTY}",
            toolchain.ndk_root.display()
        );
    }
    let clang_target = clang_target_for(&target_arch, platform_api).map_err(|error| error.to_string())?;

    point_clang_sys_at_ndk_libclang(&toolchain);
    let bindings = configure_builder(&toolchain, &clang_target, &manifest_directory)
        .generate()
        .map_err(|error| format!("bindgen не смог разобрать заголовки NDK: {error}"))?;
    bindings
        .write_to_file(out_directory.join("ndk_bindings.rs"))
        .map_err(|error| format!("не удалось записать ndk_bindings.rs: {error}"))?;

    println!("cargo:rustc-link-lib=camera2ndk");
    println!("cargo:rustc-link-lib=mediandk");
    // `sync_merge` живёт в libsync (API 26+), отдельно от camera2ndk/mediandk.
    println!("cargo:rustc-link-lib=sync");
    // `AHardwareBuffer_*` — в libnativewindow.
    println!("cargo:rustc-link-lib=nativewindow");
    Ok(())
}

/// Направляет clang-sys на libclang из NDK (clang-sys читает `LIBCLANG_PATH` при загрузке).
fn point_clang_sys_at_ndk_libclang(toolchain: &NdkToolchain) {
    // SAFETY: build.rs однопоточный; переменная выставляется до первого обращения к bindgen/clang-sys,
    // и больше никто в процессе окружение не читает и не меняет.
    unsafe {
        env::set_var("LIBCLANG_PATH", &toolchain.libclang_directory);
    }
}

fn configure_builder(toolchain: &NdkToolchain, clang_target: &str, manifest_directory: &Path) -> bindgen::Builder {
    let mut builder = bindgen::Builder::default()
        .header(manifest_directory.join("wrapper.h").to_string_lossy())
        .clang_args(clang_arguments(toolchain, clang_target))
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .rust_edition(bindgen::RustEdition::Edition2024)
        // Перечисления C — константы нужного целого типа, не Rust-enum: значение из HAL,
        // которого нет в заголовках, в Rust-enum было бы UB. Имена констант без префикса типа.
        .default_enum_style(bindgen::EnumVariation::Consts)
        .prepend_enum_name(false)
        .use_core()
        .ctypes_prefix("::core::ffi")
        .derive_default(true)
        .layout_tests(false)
        .generate_comments(false);

    for pattern in ALLOWED_FUNCTIONS {
        builder = builder.allowlist_function(pattern);
    }
    for pattern in ALLOWED_TYPES {
        builder = builder.allowlist_type(pattern);
    }
    for pattern in ALLOWED_CONSTANTS {
        builder = builder.allowlist_var(pattern);
    }
    builder
}
