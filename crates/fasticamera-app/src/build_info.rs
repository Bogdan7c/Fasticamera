//! Сведения о сборке нативной библиотеки: версия, ревизия git, ABI, debug-assertions.
//!
//! Ревизию git передаёт Gradle через переменную окружения `FASTICAMERA_GIT_REVISION`
//! при вызове cargo-ndk (`android/app/build.gradle.kts`). При сборке на ПК её нет — тогда
//! в отчёте честный `null`, а не выдуманное значение.

use serde::Serialize;

/// Что именно собрано. Значения фиксируются при компиляции.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuildInfo {
    /// Версия crate-а (`workspace.package.version`).
    pub crate_version: &'static str,
    /// Короткий SHA коммита, `-dirty` при незакоммиченных правках; `None` — сборка не из Gradle.
    pub git_revision: Option<&'static str>,
    /// ABI Android, под который скомпилирована библиотека.
    pub abi: &'static str,
    /// Включены ли debug-assertions (true для профиля `android-debug`, false для `release`).
    pub debug_assertions: bool,
}

impl BuildInfo {
    /// Сведения о текущей сборке.
    pub fn current() -> Self {
        Self {
            crate_version: env!("CARGO_PKG_VERSION"),
            git_revision: option_env!("FASTICAMERA_GIT_REVISION"),
            abi: android_abi_for_arch(std::env::consts::ARCH),
            debug_assertions: cfg!(debug_assertions),
        }
    }
}

/// Имя ABI Android для архитектуры Rust (`std::env::consts::ARCH`).
/// Неизвестная архитектура — `"unknown"`: так её видно в отчёте, и smoke-проверка честно падает.
pub fn android_abi_for_arch(arch: &str) -> &'static str {
    match arch {
        "aarch64" => "arm64-v8a",
        "arm" => "armeabi-v7a",
        "x86_64" => "x86_64",
        "x86" => "x86",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_known_architectures_to_android_abi() {
        assert_eq!(android_abi_for_arch("aarch64"), "arm64-v8a");
        assert_eq!(android_abi_for_arch("arm"), "armeabi-v7a");
        assert_eq!(android_abi_for_arch("x86_64"), "x86_64");
        assert_eq!(android_abi_for_arch("riscv64"), "unknown");
    }

    #[test]
    fn current_build_reports_crate_version_and_host_abi() {
        let info = BuildInfo::current();
        assert_eq!(info.crate_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(info.abi, android_abi_for_arch(std::env::consts::ARCH));
        assert_eq!(info.debug_assertions, cfg!(debug_assertions));
    }
}
