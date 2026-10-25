//! Сценарий `hello`: библиотека загружена и знает, что она за сборка.

use serde::Serialize;

use crate::build_info::BuildInfo;
use crate::logging::platform_write_failures;
use crate::runtime_config::{BuildType, RuntimeConfig};

/// Отчёт `hello`. `abi` вынесен наверх: его проверяет `scripts/device-smoke.sh`.
#[derive(Debug, Serialize)]
pub struct HelloReport {
    pub build: BuildInfo,
    pub abi: &'static str,
    /// Тип сборки по словам Kotlin (`BuildConfig.BUILD_TYPE`).
    pub build_type: BuildType,
    /// Действующий фильтр логов.
    pub log_filter: String,
    /// Сколько записей logcat отверг с начала процесса.
    pub log_write_failures: u64,
}

impl HelloReport {
    pub fn collect(runtime: &RuntimeConfig) -> Self {
        let build = BuildInfo::current();
        Self {
            abi: build.abi,
            build,
            build_type: runtime.build_type(),
            log_filter: runtime.log_filter().directives().to_owned(),
            log_write_failures: platform_write_failures(),
        }
    }
}
