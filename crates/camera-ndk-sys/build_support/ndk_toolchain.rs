//! Поиск закреплённого NDK и libclang для bindgen — чистая логика без обращения к окружению.
//!
//! Файл подключается дважды: из `build.rs` (`#[path]`) и из `src/lib.rs` под `cfg(test)`, чтобы
//! логику поиска можно было проверять обычным `cargo test` на ПК. Поэтому здесь только `std`;
//! всё, что зависит от процесса (переменные окружения, файловая система), приходит снаружи:
//! окружение — структурой [`ToolchainEnvironment`], проверка существования — замыканием.
//!
//! Порядок поиска повторяет `gpu-vulkan` (решение 33, glslc): явный override, затем
//! `ANDROID_NDK_HOME`, затем `<корень SDK>/ndk/<версия из gradle.properties>`.

use std::fmt;
use std::path::{Path, PathBuf};

/// Ключ версии NDK в `android/gradle.properties`.
pub const NDK_VERSION_PROPERTY: &str = "fasticamera.ndkVersion";
/// Ключ уровня API, под который собирается Rust, в `android/gradle.properties`.
pub const PLATFORM_API_PROPERTY: &str = "fasticamera.rustPlatformApi";
/// Файл, по которому узнаём, что каталог — настоящий NDK с заголовками камеры.
const PROBE_HEADER: &str = "usr/include/camera/NdkCameraManager.h";

/// Всё, что нужно для поиска NDK, без обращения к реальному окружению.
#[derive(Debug, Clone, Default)]
pub struct ToolchainEnvironment {
    /// `FASTICAMERA_LIBCLANG_PATH`: каталог с libclang (высший приоритет для libclang).
    pub libclang_override: Option<PathBuf>,
    /// `ANDROID_NDK_HOME`: корень конкретного NDK.
    pub ndk_home: Option<PathBuf>,
    /// Корни Android SDK по приоритету (`ANDROID_HOME`, `ANDROID_SDK_ROOT`, `~/Android/Sdk`).
    pub sdk_roots: Vec<PathBuf>,
    /// Версия NDK из `gradle.properties`.
    pub ndk_version: Option<String>,
    /// Уровень API из `gradle.properties` (`fasticamera.rustPlatformApi`).
    pub platform_api: Option<String>,
    /// ОС, на которой идёт сборка (`std::env::consts::OS`).
    pub host_os: String,
}

/// Найденный и проверенный NDK: всё, что нужно передать bindgen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NdkToolchain {
    /// Корень NDK.
    pub ndk_root: PathBuf,
    /// Каталог с libclang (в него смотрит `LIBCLANG_PATH`).
    pub libclang_directory: PathBuf,
    /// `--sysroot` для clang.
    pub sysroot: PathBuf,
    /// Имя каталога корня NDK совпадает с закреплённой версией. `false` — предупреждение:
    /// собираем по другому NDK, чем в Gradle.
    pub matches_pinned_version: bool,
}

/// Что не так с одним проверенным кандидатом.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateReport {
    /// Проверенный корень NDK.
    pub ndk_root: PathBuf,
    /// Чего в нём не хватило.
    pub missing: Vec<PathBuf>,
}

/// Почему NDK или libclang не найдены.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolchainError {
    /// `FASTICAMERA_LIBCLANG_PATH` задан, но libclang в нём нет: владелец указал путь намеренно,
    /// молча искать дальше нельзя.
    LibclangOverrideInvalid {
        /// Что задано.
        directory: PathBuf,
        /// Какой файл ожидался.
        expected: PathBuf,
    },
    /// Ни один кандидат не подошёл.
    NdkNotFound {
        /// Проверенные кандидаты и чего не хватило каждому.
        reports: Vec<CandidateReport>,
        /// Есть ли версия NDK в gradle.properties (иначе SDK-кандидатов нет вовсе).
        version_known: bool,
    },
    /// В `gradle.properties` нет уровня API.
    PlatformApiMissing,
    /// Архитектура цели не из поддерживаемых Android ABI.
    UnsupportedArchitecture(String),
}

impl fmt::Display for ToolchainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolchainError::LibclangOverrideInvalid { directory, expected } => write!(
                formatter,
                "FASTICAMERA_LIBCLANG_PATH={} задан, но там нет {}.\n\
                 Что сделать: укажите каталог, где лежит libclang, либо уберите переменную — тогда libclang \
                 берётся из закреплённого NDK",
                directory.display(),
                expected.display()
            ),
            ToolchainError::NdkNotFound { reports, version_known } => {
                writeln!(
                    formatter,
                    "не найден закреплённый NDK с заголовками Camera2 и libclang."
                )?;
                if reports.is_empty() {
                    writeln!(
                        formatter,
                        "Проверять нечего: ни ANDROID_NDK_HOME, ни корней SDK с версией NDK."
                    )?;
                }
                if !version_known {
                    writeln!(
                        formatter,
                        "В android/gradle.properties не прочитан ключ {NDK_VERSION_PROPERTY}: \
                         поиск в <SDK>/ndk/<версия> невозможен."
                    )?;
                }
                for report in reports {
                    writeln!(formatter, "Проверено {}, не хватает:", report.ndk_root.display())?;
                    for missing in &report.missing {
                        writeln!(formatter, "  - {}", missing.display())?;
                    }
                }
                write!(
                    formatter,
                    "Что сделать: установите NDK версии из android/gradle.properties ({NDK_VERSION_PROPERTY}) \
                     в ~/Android/Sdk/ndk/<версия>, либо задайте ANDROID_NDK_HOME=/путь/к/ndk, \
                     либо FASTICAMERA_LIBCLANG_PATH=/каталог/с/libclang вместе с ANDROID_NDK_HOME"
                )
            }
            ToolchainError::PlatformApiMissing => write!(
                formatter,
                "в android/gradle.properties нет ключа {PLATFORM_API_PROPERTY}: неизвестно, под какой уровень API \
                 разбирать заголовки NDK"
            ),
            ToolchainError::UnsupportedArchitecture(architecture) => write!(
                formatter,
                "архитектура цели {architecture} не соответствует ни одному Android ABI (aarch64, arm, x86_64, x86)"
            ),
        }
    }
}

/// Достаёт значение ключа из текста `gradle.properties` (формат `ключ=значение`, `#` — комментарий).
pub fn parse_gradle_property(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.trim();
        if line.starts_with('#') || line.starts_with('!') {
            return None;
        }
        let (name, value) = line.split_once('=')?;
        (name.trim() == key).then(|| value.trim().to_owned())
    })
}

/// Имя каталога хоста внутри NDK для ОС сборки.
pub fn host_tag_for(operating_system: &str) -> &'static str {
    match operating_system {
        "macos" => "darwin-x86_64",
        "windows" => "windows-x86_64",
        _ => "linux-x86_64",
    }
}

/// Имя файла libclang на ОС сборки.
fn libclang_file_name_for(operating_system: &str) -> &'static str {
    match operating_system {
        "macos" => "libclang.dylib",
        "windows" => "libclang.dll",
        _ => "libclang.so",
    }
}

/// Подкаталог NDK, где лежит libclang: на Windows DLL живут в `bin`, на остальных в `lib`.
fn libclang_subdirectory_for(operating_system: &str) -> &'static str {
    match operating_system {
        "windows" => "bin",
        _ => "lib",
    }
}

/// Уровень API из окружения; нет ключа в `gradle.properties` — ошибка (угадывать уровень нельзя).
pub fn required_platform_api(environment: &ToolchainEnvironment) -> Result<&str, ToolchainError> {
    environment
        .platform_api
        .as_deref()
        .ok_or(ToolchainError::PlatformApiMissing)
}

/// Тройка clang для `--target` (с уровнем API): `aarch64-linux-android36`.
///
/// `target_arch` — значение `CARGO_CFG_TARGET_ARCH`. Для 32-битной ARM clang ожидает
/// `armv7a-linux-androideabi`, для x86 — `i686-linux-android`.
pub fn clang_target_for(target_arch: &str, platform_api: &str) -> Result<String, ToolchainError> {
    let base = match target_arch {
        "aarch64" => "aarch64-linux-android",
        "arm" => "armv7a-linux-androideabi",
        "x86_64" => "x86_64-linux-android",
        "x86" => "i686-linux-android",
        other => return Err(ToolchainError::UnsupportedArchitecture(other.to_owned())),
    };
    Ok(format!("{base}{platform_api}"))
}

/// Каталог `toolchains/llvm/prebuilt/<хост>` внутри корня NDK.
fn llvm_directory(environment: &ToolchainEnvironment, ndk_root: &Path) -> PathBuf {
    ndk_root
        .join("toolchains")
        .join("llvm")
        .join("prebuilt")
        .join(host_tag_for(&environment.host_os))
}

/// Кандидаты в корни NDK по приоритету (без проверки существования).
///
/// Порядок: `ANDROID_NDK_HOME` → `<корень SDK>/ndk/<версия>` по каждому корню SDK.
/// Версия берётся только из `gradle.properties` — в коде её нет.
pub fn ndk_root_candidates(environment: &ToolchainEnvironment) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(ndk_home) = &environment.ndk_home {
        candidates.push(ndk_home.clone());
    }
    if let Some(version) = &environment.ndk_version {
        for sdk_root in &environment.sdk_roots {
            candidates.push(sdk_root.join("ndk").join(version));
        }
    }
    candidates
}

/// Чего не хватает кандидату: заголовка камеры и (если нет override) libclang из его набора.
fn missing_parts(environment: &ToolchainEnvironment, ndk_root: &Path, exists: &dyn Fn(&Path) -> bool) -> Vec<PathBuf> {
    let llvm = llvm_directory(environment, ndk_root);
    let mut required = vec![llvm.join("sysroot").join(PROBE_HEADER)];
    if environment.libclang_override.is_none() {
        required.push(
            llvm.join(libclang_subdirectory_for(&environment.host_os))
                .join(libclang_file_name_for(&environment.host_os)),
        );
    }
    required.into_iter().filter(|path| !exists(path)).collect()
}

/// Находит NDK и каталог libclang.
///
/// `FASTICAMERA_LIBCLANG_PATH` задаёт только libclang; заголовки всё равно берутся из NDK.
/// Override на каталог без libclang — ошибка, а не повод молча искать дальше: путь задан намеренно.
pub fn resolve_toolchain(
    environment: &ToolchainEnvironment,
    exists: &dyn Fn(&Path) -> bool,
) -> Result<NdkToolchain, ToolchainError> {
    let libclang_name = libclang_file_name_for(&environment.host_os);
    if let Some(directory) = &environment.libclang_override {
        let expected = directory.join(libclang_name);
        if !exists(&expected) {
            return Err(ToolchainError::LibclangOverrideInvalid {
                directory: directory.clone(),
                expected,
            });
        }
    }

    let mut reports = Vec::new();
    for ndk_root in ndk_root_candidates(environment) {
        let missing = missing_parts(environment, &ndk_root, exists);
        if !missing.is_empty() {
            reports.push(CandidateReport { ndk_root, missing });
            continue;
        }
        let llvm = llvm_directory(environment, &ndk_root);
        let libclang_directory = match &environment.libclang_override {
            Some(directory) => directory.clone(),
            None => llvm.join(libclang_subdirectory_for(&environment.host_os)),
        };
        let matches_pinned_version = match (&environment.ndk_version, ndk_root.file_name()) {
            (Some(version), Some(name)) => name.to_string_lossy() == version.as_str(),
            _ => false,
        };
        return Ok(NdkToolchain {
            sysroot: llvm.join("sysroot"),
            ndk_root,
            libclang_directory,
            matches_pinned_version,
        });
    }
    Err(ToolchainError::NdkNotFound {
        reports,
        version_known: environment.ndk_version.is_some(),
    })
}

/// Аргументы clang для bindgen: цель с уровнем API и sysroot из NDK.
pub fn clang_arguments(toolchain: &NdkToolchain, clang_target: &str) -> Vec<String> {
    vec![
        format!("--target={clang_target}"),
        format!("--sysroot={}", toolchain.sysroot.display()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment() -> ToolchainEnvironment {
        ToolchainEnvironment {
            libclang_override: None,
            ndk_home: None,
            sdk_roots: vec![PathBuf::from("/sdk-a"), PathBuf::from("/sdk-b")],
            ndk_version: Some("30.0.1".to_owned()),
            platform_api: Some("36".to_owned()),
            host_os: "linux".to_owned(),
        }
    }

    /// Файлы «полного» NDK в корне `root`.
    fn full_ndk(root: &str) -> Vec<PathBuf> {
        let llvm = Path::new(root).join("toolchains/llvm/prebuilt/linux-x86_64");
        vec![
            llvm.join("sysroot/usr/include/camera/NdkCameraManager.h"),
            llvm.join("lib/libclang.so"),
        ]
    }

    fn exists_in(files: Vec<PathBuf>) -> impl Fn(&Path) -> bool {
        move |path: &Path| files.iter().any(|file| file == path)
    }

    #[test]
    fn parses_properties_and_ignores_comments() {
        let text = "# c\nfasticamera.ndkVersion = 30.0.16248370 \nfasticamera.rustPlatformApi=36\n#fasticamera.x=1\n";
        assert_eq!(
            parse_gradle_property(text, NDK_VERSION_PROPERTY).as_deref(),
            Some("30.0.16248370")
        );
        assert_eq!(
            parse_gradle_property(text, PLATFORM_API_PROPERTY).as_deref(),
            Some("36")
        );
        assert_eq!(parse_gradle_property(text, "fasticamera.x"), None);
    }

    #[test]
    fn real_gradle_properties_pin_ndk_and_api() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../android/gradle.properties");
        let text = std::fs::read_to_string(&path).unwrap();
        let version = parse_gradle_property(&text, NDK_VERSION_PROPERTY).unwrap();
        assert!(version.split('.').count() >= 3, "версия NDK странная: {version}");
        let api = parse_gradle_property(&text, PLATFORM_API_PROPERTY).unwrap();
        assert!(api.parse::<u32>().is_ok(), "уровень API не число: {api}");
    }

    #[test]
    fn ndk_home_comes_before_sdk_roots_and_roots_keep_order() {
        let mut env = environment();
        env.ndk_home = Some(PathBuf::from("/ndk"));
        assert_eq!(
            ndk_root_candidates(&env),
            vec![
                PathBuf::from("/ndk"),
                PathBuf::from("/sdk-a/ndk/30.0.1"),
                PathBuf::from("/sdk-b/ndk/30.0.1")
            ]
        );
    }

    #[test]
    fn without_ndk_home_the_sdk_fallback_is_used_like_clippy_android_stage_does() {
        // Стадия clippy-android идёт без cargo-ndk и без ANDROID_NDK_HOME (CI).
        let env = environment();
        let toolchain = resolve_toolchain(&env, &exists_in(full_ndk("/sdk-b/ndk/30.0.1"))).unwrap();
        assert_eq!(toolchain.ndk_root, PathBuf::from("/sdk-b/ndk/30.0.1"));
        assert_eq!(
            toolchain.libclang_directory,
            PathBuf::from("/sdk-b/ndk/30.0.1/toolchains/llvm/prebuilt/linux-x86_64/lib")
        );
        assert_eq!(
            toolchain.sysroot,
            PathBuf::from("/sdk-b/ndk/30.0.1/toolchains/llvm/prebuilt/linux-x86_64/sysroot")
        );
        assert!(toolchain.matches_pinned_version);
    }

    #[test]
    fn ndk_home_with_another_version_is_used_but_flagged() {
        let mut env = environment();
        env.ndk_home = Some(PathBuf::from("/opt/ndk-29"));
        let toolchain = resolve_toolchain(&env, &exists_in(full_ndk("/opt/ndk-29"))).unwrap();
        assert_eq!(toolchain.ndk_root, PathBuf::from("/opt/ndk-29"));
        assert!(!toolchain.matches_pinned_version);
    }

    #[test]
    fn broken_first_candidate_falls_through_to_next_and_is_not_reported_on_success() {
        let mut env = environment();
        env.ndk_home = Some(PathBuf::from("/broken"));
        let toolchain = resolve_toolchain(&env, &exists_in(full_ndk("/sdk-a/ndk/30.0.1"))).unwrap();
        assert_eq!(toolchain.ndk_root, PathBuf::from("/sdk-a/ndk/30.0.1"));
    }

    #[test]
    fn libclang_override_replaces_ndk_libclang_but_headers_still_come_from_ndk() {
        let mut env = environment();
        env.libclang_override = Some(PathBuf::from("/custom"));
        // В NDK нет собственного libclang — override это покрывает.
        let mut files = vec![PathBuf::from("/custom/libclang.so")];
        files.push(full_ndk("/sdk-a/ndk/30.0.1")[0].clone());
        let toolchain = resolve_toolchain(&env, &exists_in(files)).unwrap();
        assert_eq!(toolchain.libclang_directory, PathBuf::from("/custom"));
        assert_eq!(toolchain.ndk_root, PathBuf::from("/sdk-a/ndk/30.0.1"));
    }

    #[test]
    fn libclang_override_without_library_is_an_error_not_a_fallback() {
        let mut env = environment();
        env.libclang_override = Some(PathBuf::from("/custom"));
        let error = resolve_toolchain(&env, &exists_in(full_ndk("/sdk-a/ndk/30.0.1"))).unwrap_err();
        assert_eq!(
            error,
            ToolchainError::LibclangOverrideInvalid {
                directory: PathBuf::from("/custom"),
                expected: PathBuf::from("/custom/libclang.so"),
            }
        );
    }

    #[test]
    fn not_found_lists_every_checked_candidate_with_its_missing_files() {
        let mut env = environment();
        env.ndk_home = Some(PathBuf::from("/ndk"));
        // В /sdk-a/ndk/30.0.1 есть заголовок, но нет libclang.
        let partial = vec![full_ndk("/sdk-a/ndk/30.0.1")[0].clone()];
        let error = resolve_toolchain(&env, &exists_in(partial)).unwrap_err();
        let ToolchainError::NdkNotFound { reports, version_known } = &error else {
            panic!("ожидалась NdkNotFound, получено {error:?}");
        };
        assert!(*version_known);
        assert_eq!(reports.len(), 3);
        assert_eq!(reports[0].ndk_root, PathBuf::from("/ndk"));
        assert_eq!(reports[0].missing.len(), 2, "в /ndk нет ни заголовка, ни libclang");
        assert_eq!(reports[1].ndk_root, PathBuf::from("/sdk-a/ndk/30.0.1"));
        assert_eq!(
            reports[1].missing,
            vec![PathBuf::from(
                "/sdk-a/ndk/30.0.1/toolchains/llvm/prebuilt/linux-x86_64/lib/libclang.so"
            )]
        );
        let text = error.to_string();
        assert!(
            text.contains("/sdk-b/ndk/30.0.1"),
            "сообщение перечисляет кандидатов: {text}"
        );
        assert!(
            text.contains("ANDROID_NDK_HOME"),
            "сообщение говорит, что сделать: {text}"
        );
    }

    #[test]
    fn missing_version_is_reported_when_no_candidates_exist() {
        let mut env = environment();
        env.ndk_version = None;
        let error = resolve_toolchain(&env, &|_: &Path| true).unwrap_err();
        assert_eq!(
            error,
            ToolchainError::NdkNotFound {
                reports: Vec::new(),
                version_known: false
            }
        );
        assert!(error.to_string().contains(NDK_VERSION_PROPERTY));
    }

    #[test]
    fn clang_target_matches_ndk_triples_for_every_android_abi() {
        assert_eq!(clang_target_for("aarch64", "36").unwrap(), "aarch64-linux-android36");
        assert_eq!(clang_target_for("arm", "36").unwrap(), "armv7a-linux-androideabi36");
        assert_eq!(clang_target_for("x86_64", "36").unwrap(), "x86_64-linux-android36");
        assert_eq!(clang_target_for("x86", "36").unwrap(), "i686-linux-android36");
        assert_eq!(
            clang_target_for("riscv64", "36"),
            Err(ToolchainError::UnsupportedArchitecture("riscv64".to_owned()))
        );
    }

    #[test]
    fn platform_api_comes_from_properties_and_its_absence_is_an_error() {
        let env = environment();
        assert_eq!(required_platform_api(&env), Ok("36"));
        let mut without_api = environment();
        without_api.platform_api = None;
        assert_eq!(
            required_platform_api(&without_api),
            Err(ToolchainError::PlatformApiMissing)
        );
        assert!(
            ToolchainError::PlatformApiMissing
                .to_string()
                .contains(PLATFORM_API_PROPERTY)
        );
    }

    #[test]
    fn clang_arguments_carry_target_and_sysroot() {
        let env = environment();
        let toolchain = resolve_toolchain(&env, &exists_in(full_ndk("/sdk-a/ndk/30.0.1"))).unwrap();
        assert_eq!(
            clang_arguments(&toolchain, "aarch64-linux-android36"),
            vec![
                "--target=aarch64-linux-android36".to_owned(),
                "--sysroot=/sdk-a/ndk/30.0.1/toolchains/llvm/prebuilt/linux-x86_64/sysroot".to_owned()
            ]
        );
    }

    #[test]
    fn windows_host_looks_for_dll_in_bin() {
        let mut env = environment();
        env.host_os = "windows".to_owned();
        let llvm = "/sdk-a/ndk/30.0.1/toolchains/llvm/prebuilt/windows-x86_64";
        let files = vec![
            PathBuf::from(format!("{llvm}/sysroot/usr/include/camera/NdkCameraManager.h")),
            PathBuf::from(format!("{llvm}/bin/libclang.dll")),
        ];
        let toolchain = resolve_toolchain(&env, &exists_in(files)).unwrap();
        assert_eq!(toolchain.libclang_directory, PathBuf::from(format!("{llvm}/bin")));
    }
}
