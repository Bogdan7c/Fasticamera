# Tech stack (pins — источник: user/foundation/results/00.md, сессия 01)

- Rust 1.97.1 (закреплён `rust-toolchain.toml`, компоненты rustfmt/clippy, таргет `aarch64-linux-android`), edition 2024, resolver 3. Cargo workspace в корне репо: `members = ["crates/*"]`; версии внешних зависимостей только в `[workspace.dependencies]`.
- Android: AGP 9.4.1 (встроенный Kotlin, без отдельного kotlin-плагина), Gradle 9.8.2 wrapper (`android/gradlew`), JDK 21, SDK `~/Android/Sdk`, NDK 30.0.16248370, cargo-ndk 4.1.2. compile/min/targetSdk = 36, ABI только arm64-v8a. Без AndroidX/Compose (пока).
- Rust → APK: задача `cargoNdkBuild<Variant>` в `android/app/build.gradle.kts` (cargo-ndk `-P 36 --locked -p fasticamera-app --profile <из gradle.properties>`), подключена через Variant API `sources.jniLibs.addGeneratedSourceDirectory`. Gradle передаёт `FASTICAMERA_GIT_REVISION` (sha + `-dirty`, включая неотслеживаемые файлы).
- Cargo-профили: `android-debug` (inherits dev, opt 1, debug full) для debug APK; `release` (fat LTO, cgu 1, line-tables) для release. `panic = "unwind"` везде — паника ловится на границе JNI (решение владельца). AGP урезает .so в APK; полные символы: `android/app/build/outputs/native-debug-symbols/<variant>/`.
- Варианты APK стоят рядом: debug = `dev.fasticamera.debug`, release = `dev.fasticamera` (подписан debug-ключом). Namespace/JNI-класс всегда `dev.fasticamera`.
- Rust-зависимости продукта: tracing 0.1, tracing-subscriber 0.3 (только `std`+`registry`, без fmt/EnvFilter), serde/serde_json, thiserror 2; Android-only: jni 0.22.4 (`native_method!`, RegisterNatives), android_log-sys 0.3.2, ndk-sys 0.6.0 (фича `nativewindow`; только в `android-shell`; `AThermal_*` объявлены сами в `thermal_ffi.rs`, т.к. в 0.6 их нет). Крейт `ndk` не используется (нет датчиков/тепла, другой `jni-sys`).
- Манифест: `CAMERA`, `VIBRATE`, `HIGH_SAMPLING_RATE_SENSORS` (гироскоп > 200 Гц); Activity `screenOrientation=portrait`, `configChanges`.
- clippy 1.97 даёт ложный `missing_const_for_thread_local` только для `aarch64-linux-android` — подавлен точечным `cfg_attr(android, expect(...))` в `panic_report.rs`. Мост tracing → logcat свой (решение владельца: готовые мосты молодые).
- Python 3.14 stdlib (tomllib) для скриптов проверок; без pip-пакетов.
- Зонд `tools/device-probe` — отдельный Gradle+Cargo проект со своим lock-файлом, вне workspace (ash 0.38, ndk-sys 0.6).
