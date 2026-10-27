//! Сборка шейдеров GLSL → SPIR-V (glslc и spirv-val из NDK): поиск инструментов и компиляция каталога.
//!
//! # Кто подключает файл
//! * `crates/gpu-vulkan/build.rs` — `#[path = "build_support/shader_tools.rs"]`;
//! * `crates/isp-vulkan/build.rs` (сессия 07) — `#[path = "../gpu-vulkan/build_support/shader_tools.rs"]`;
//! * `crates/gpu-vulkan/src/shaders.rs` под `cfg(test)` — чтобы чистая логика поиска тестировалась
//!   обычным `cargo test`.
//!
//! Поэтому файл САМОДОСТАТОЧЕН: только `std`, никаких `crate::`/`super::`, никаких путей относительно
//! конкретного crate. Корень workspace ищется подъёмом от `CARGO_MANIFEST_DIR` вызывающего до каталога,
//! где лежит `android/gradle.properties` ([`find_workspace_root`]); версия NDK берётся оттуда
//! (`fasticamera.ndkVersion`), в коде её нет.
//!
//! # Использование из build.rs
//! ```ignore
//! #[path = "../gpu-vulkan/build_support/shader_tools.rs"]
//! mod shader_tools;
//! fn main() {
//!     if let Err(message) = shader_tools::build_shader_blobs_from_cargo_env() {
//!         eprintln!("\nошибка сборки шейдеров: {message}\n");
//!         std::process::exit(1);
//!     }
//! }
//! ```
//! Результат — `$OUT_DIR/shader_blobs.rs` с константами `NAME_STAGE: &[u8]` (`name.comp` →
//! `NAME_COMP`), подключается `include!(concat!(env!("OUT_DIR"), "/shader_blobs.rs"))`.
//!
//! Чистая логика (поиск путей, имена констант) не обращается к окружению процесса: всё приходит через
//! [`ToolchainEnvironment`]. Окружение читают только функции раздела «сборка каталога».

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

/// Каталог инструментов шейдеров внутри NDK (после каталога версии).
pub const SHADER_TOOLS_DIRECTORY: &str = "shader-tools";
/// Имя ключа версии NDK в `android/gradle.properties`.
pub const NDK_VERSION_PROPERTY: &str = "fasticamera.ndkVersion";

/// Всё, что нужно для поиска glslc, без обращения к реальному окружению.
#[derive(Debug, Clone, Default)]
pub struct ToolchainEnvironment {
    /// `FASTICAMERA_GLSLC`: явный путь к glslc (высший приоритет).
    pub glslc_override: Option<PathBuf>,
    /// `ANDROID_NDK_HOME`: корень конкретного NDK.
    pub ndk_home: Option<PathBuf>,
    /// Корни Android SDK по приоритету (`ANDROID_HOME`, `ANDROID_SDK_ROOT`, `~/Android/Sdk`).
    pub sdk_roots: Vec<PathBuf>,
    /// Версия NDK из `gradle.properties` (`None`, если файла/ключа нет).
    pub ndk_version: Option<String>,
    /// Имя каталога хоста внутри NDK, например `linux-x86_64`.
    pub host_tag: String,
}

/// Почему инструмент не найден.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    /// glslc не найден ни одним способом; перечислены проверенные пути.
    GlslcNotFound {
        /// Что проверяли.
        tried: Vec<PathBuf>,
    },
    /// glslc найден, но рядом нет нужного инструмента (spirv-val).
    SiblingMissing {
        /// Какой инструмент.
        tool: String,
        /// Где искали.
        expected: PathBuf,
    },
}

impl fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolError::GlslcNotFound { tried } => {
                writeln!(formatter, "не найден компилятор шейдеров glslc из NDK.")?;
                writeln!(formatter, "Проверено:")?;
                for path in tried {
                    writeln!(formatter, "  - {}", path.display())?;
                }
                write!(
                    formatter,
                    "Что сделать: установите NDK версии из android/gradle.properties ({NDK_VERSION_PROPERTY}) \
                     или задайте FASTICAMERA_GLSLC=/путь/к/glslc либо ANDROID_NDK_HOME=/путь/к/ndk"
                )
            }
            ToolError::SiblingMissing { tool, expected } => write!(
                formatter,
                "рядом с glslc нет {tool} (ожидался {}); нужен полный каталог {SHADER_TOOLS_DIRECTORY} NDK",
                expected.display()
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

/// Имя каталога хоста внутри NDK для текущей ОС сборки.
pub fn host_tag_for(operating_system: &str) -> &'static str {
    match operating_system {
        "macos" => "darwin-x86_64",
        "windows" => "windows-x86_64",
        _ => "linux-x86_64",
    }
}

/// Список кандидатов на путь к glslc в порядке приоритета (без проверки существования).
///
/// Порядок: явный override → `ANDROID_NDK_HOME` → `<sdk>/ndk/<версия>` по каждому корню SDK.
/// Версия NDK берётся только из `gradle.properties` — в коде её нет.
pub fn glslc_candidates(environment: &ToolchainEnvironment) -> Vec<PathBuf> {
    let tool_path = |ndk_root: &Path| {
        ndk_root
            .join(SHADER_TOOLS_DIRECTORY)
            .join(&environment.host_tag)
            .join("glslc")
    };
    let mut candidates = Vec::new();
    if let Some(explicit) = &environment.glslc_override {
        candidates.push(explicit.clone());
    }
    if let Some(ndk_home) = &environment.ndk_home {
        candidates.push(tool_path(ndk_home));
    }
    if let Some(version) = &environment.ndk_version {
        for sdk_root in &environment.sdk_roots {
            candidates.push(tool_path(&sdk_root.join("ndk").join(version)));
        }
    }
    candidates
}

/// Выбирает первый существующий glslc из кандидатов.
///
/// Явный override, указывающий на несуществующий файл, — ошибка, а не повод молча искать дальше:
/// владелец задал путь намеренно.
pub fn resolve_glslc(environment: &ToolchainEnvironment, exists: &dyn Fn(&Path) -> bool) -> Result<PathBuf, ToolError> {
    let candidates = glslc_candidates(environment);
    if let Some(explicit) = &environment.glslc_override {
        return if exists(explicit) {
            Ok(explicit.clone())
        } else {
            Err(ToolError::GlslcNotFound {
                tried: vec![explicit.clone()],
            })
        };
    }
    match candidates.iter().find(|candidate| exists(candidate)) {
        Some(found) => Ok(found.clone()),
        None => Err(ToolError::GlslcNotFound { tried: candidates }),
    }
}

/// Путь к соседнему инструменту (например, `spirv-val`) в каталоге glslc.
pub fn sibling_tool(glslc: &Path, tool: &str, exists: &dyn Fn(&Path) -> bool) -> Result<PathBuf, ToolError> {
    let directory = glslc.parent().unwrap_or_else(|| Path::new("."));
    let expected = directory.join(tool);
    if exists(&expected) {
        Ok(expected)
    } else {
        Err(ToolError::SiblingMissing {
            tool: tool.to_owned(),
            expected,
        })
    }
}

/// Имя Rust-константы для файла шейдера: `fullscreen_triangle.vert` → `FULLSCREEN_TRIANGLE_VERT`.
/// `None` — если расширение не шейдерное.
pub fn constant_name_for(file_name: &str) -> Option<String> {
    let (stem, extension) = file_name.rsplit_once('.')?;
    if !matches!(extension, "vert" | "frag" | "comp") || stem.is_empty() {
        return None;
    }
    let sanitized: String = stem
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    Some(format!("{sanitized}_{}", extension.to_ascii_uppercase()))
}

// ---- сборка каталога шейдеров (читает окружение процесса, запускает инструменты) ----

/// Целевая среда SPIR-V: Vulkan 1.3 (решение сессии 03).
pub const TARGET_ENVIRONMENT: &str = "vulkan1.3";
/// Имя генерируемого файла с константами в `OUT_DIR`.
pub const BLOBS_FILE_NAME: &str = "shader_blobs.rs";
/// Путь к `gradle.properties` относительно корня workspace.
pub const GRADLE_PROPERTIES_RELATIVE: &str = "android/gradle.properties";

/// Ищет корень workspace: первый каталог вверх от `start` (включая его), где есть
/// `android/gradle.properties`. `None` — так высоко файла нет (сборка вне репозитория).
pub fn find_workspace_root(start: &Path, exists: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|candidate| exists(&candidate.join(GRADLE_PROPERTIES_RELATIVE)))
        .map(Path::to_path_buf)
}

/// Читает окружение процесса в структуру для чистой логики поиска.
pub fn read_environment(gradle_properties: Option<&Path>) -> ToolchainEnvironment {
    let path_variable = |name: &str| env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from);
    let mut sdk_roots: Vec<PathBuf> = ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
        .iter()
        .filter_map(|name| path_variable(name))
        .collect();
    if let Some(home) = path_variable("HOME") {
        sdk_roots.push(home.join("Android").join("Sdk"));
    }
    let ndk_version = gradle_properties
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| parse_gradle_property(&text, NDK_VERSION_PROPERTY));
    ToolchainEnvironment {
        glslc_override: path_variable("FASTICAMERA_GLSLC"),
        ndk_home: path_variable("ANDROID_NDK_HOME"),
        sdk_roots,
        ndk_version,
        host_tag: host_tag_for(env::consts::OS).to_owned(),
    }
}

/// Исходники шейдеров каталога (отсортированы — сборка детерминирована).
pub fn list_shader_sources(shaders_directory: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = fs::read_dir(shaders_directory)
        .map_err(|error| format!("не удалось прочитать {}: {error}", shaders_directory.display()))?;
    let mut sources: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| constant_name_for(name).is_some())
        })
        .collect();
    sources.sort();
    Ok(sources)
}

/// Запускает внешний инструмент; при ошибке возвращает его вывод (stdout и stderr).
fn run_tool(mut command: Command, description: &str) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|error| format!("{description}: не удалось запустить: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{description} завершился с ошибкой ({}):\n{}{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

/// Компилирует все `*.vert|*.frag|*.comp` каталога `shaders_directory` (общие `*.glsl` подключаются
/// `#include` из него же), проверяет `spirv-val` и пишет `out_directory/shader_blobs.rs`.
pub fn compile_shader_directory(
    shaders_directory: &Path,
    out_directory: &Path,
    environment: &ToolchainEnvironment,
) -> Result<(), String> {
    let exists = |path: &Path| path.is_file();
    let glslc = resolve_glslc(environment, &exists).map_err(|error| error.to_string())?;
    let spirv_val = sibling_tool(&glslc, "spirv-val", &exists).map_err(|error| error.to_string())?;

    let mut generated =
        String::from("// Сгенерировано build.rs: SPIR-V шейдеров, вшитый в бинарь. Не редактировать.\n\n");
    for source in list_shader_sources(shaders_directory)? {
        let file_name = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("странное имя файла шейдера: {}", source.display()))?;
        let constant = constant_name_for(file_name).ok_or_else(|| format!("не шейдер: {file_name}"))?;
        let spirv_path = out_directory.join(format!("{file_name}.spv"));

        let mut compile = Command::new(&glslc);
        compile
            .arg(format!("--target-env={TARGET_ENVIRONMENT}"))
            .arg("-Werror")
            .arg("-O")
            .arg("-I")
            .arg(shaders_directory)
            .arg("-o")
            .arg(&spirv_path)
            .arg(&source);
        run_tool(compile, &format!("glslc {file_name}"))?;

        let mut validate = Command::new(&spirv_val);
        // spirv-val принимает значение --target-env отдельным аргументом (форма с `=` не работает).
        validate.arg("--target-env").arg(TARGET_ENVIRONMENT).arg(&spirv_path);
        run_tool(validate, &format!("spirv-val {file_name}"))?;

        generated.push_str(&format!(
            "/// SPIR-V шейдера `{file_name}`.\npub const {constant}: &[u8] = include_bytes!({:?});\n",
            spirv_path.display().to_string()
        ));
    }
    fs::write(out_directory.join(BLOBS_FILE_NAME), generated)
        .map_err(|error| format!("не удалось записать {BLOBS_FILE_NAME}: {error}"))
}

/// Абсолютный путь к этому файлу по `file!()`: как есть, от корня workspace или от каталога crate.
fn locate_this_file(workspace_root: Option<&Path>, manifest_directory: &Path) -> Option<PathBuf> {
    let reported = Path::new(file!());
    if reported.is_absolute() {
        return Some(reported.to_path_buf());
    }
    workspace_root
        .into_iter()
        .chain(std::iter::once(manifest_directory))
        .map(|base| base.join(reported))
        .find(|candidate| candidate.is_file())
}

/// Полный шаг build.rs: каталог `shaders/` рядом с `Cargo.toml` вызывающего crate → `$OUT_DIR/shader_blobs.rs`.
/// Печатает `cargo:rerun-if-*`, чтобы пересборка шла при смене шейдеров, NDK или переменных окружения.
pub fn build_shader_blobs_from_cargo_env() -> Result<(), String> {
    let manifest_directory = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("нет CARGO_MANIFEST_DIR")?);
    let out_directory = PathBuf::from(env::var_os("OUT_DIR").ok_or("нет OUT_DIR")?);
    let shaders_directory = manifest_directory.join("shaders");
    let workspace_root = find_workspace_root(&manifest_directory, &|path: &Path| path.is_file());
    let gradle_properties = workspace_root
        .as_ref()
        .map(|root| root.join(GRADLE_PROPERTIES_RELATIVE));

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", shaders_directory.display());
    // Сам этот файл: правка общей логики сборки пересобирает шейдеры у всех подключивших crate.
    // `file!()` относителен каталогу, из которого cargo запустил rustc (корень workspace или crate), а
    // `rerun-if-changed` cargo понимает относительно crate, поэтому печатаем абсолютный существующий путь.
    match locate_this_file(workspace_root.as_deref(), &manifest_directory) {
        Some(path) => println!("cargo:rerun-if-changed={}", path.display()),
        None => println!("cargo:warning=shader_tools.rs не найден по file!() = {}", file!()),
    }
    if let Some(path) = &gradle_properties {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    for variable in [
        "FASTICAMERA_GLSLC",
        "ANDROID_NDK_HOME",
        "ANDROID_HOME",
        "ANDROID_SDK_ROOT",
        "HOME",
    ] {
        println!("cargo:rerun-if-env-changed={variable}");
    }
    let environment = read_environment(gradle_properties.as_deref());
    compile_shader_directory(&shaders_directory, &out_directory, &environment)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment() -> ToolchainEnvironment {
        ToolchainEnvironment {
            glslc_override: None,
            ndk_home: None,
            sdk_roots: vec![PathBuf::from("/sdk-a"), PathBuf::from("/sdk-b")],
            ndk_version: Some("30.0.1".to_owned()),
            host_tag: "linux-x86_64".to_owned(),
        }
    }

    #[test]
    fn parses_ndk_version_from_gradle_properties_text() {
        let text = "# комментарий\norg.gradle.jvmargs=-Xmx2g\nfasticamera.ndkVersion = 30.0.16248370 \nother=1\n";
        assert_eq!(
            parse_gradle_property(text, NDK_VERSION_PROPERTY).as_deref(),
            Some("30.0.16248370")
        );
        assert_eq!(parse_gradle_property(text, "other").as_deref(), Some("1"));
        assert_eq!(parse_gradle_property(text, "missing"), None);
        assert_eq!(
            parse_gradle_property("#fasticamera.ndkVersion=1", NDK_VERSION_PROPERTY),
            None
        );
    }

    #[test]
    fn real_gradle_properties_of_the_repo_contains_ndk_version() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../android/gradle.properties");
        let text = std::fs::read_to_string(&path).unwrap();
        let version = parse_gradle_property(&text, NDK_VERSION_PROPERTY).unwrap();
        assert!(version.split('.').count() >= 3, "версия NDK странная: {version}");
    }

    #[test]
    fn override_has_priority_and_missing_override_is_an_error_not_a_fallback() {
        let mut env = environment();
        env.glslc_override = Some(PathBuf::from("/custom/glslc"));
        env.ndk_home = Some(PathBuf::from("/ndk"));
        let everything_exists = |_: &Path| true;
        assert_eq!(
            resolve_glslc(&env, &everything_exists).unwrap(),
            PathBuf::from("/custom/glslc")
        );
        let nothing_but_ndk = |path: &Path| path.starts_with("/ndk");
        let error = resolve_glslc(&env, &nothing_but_ndk).unwrap_err();
        assert_eq!(
            error,
            ToolError::GlslcNotFound {
                tried: vec![PathBuf::from("/custom/glslc")]
            }
        );
    }

    #[test]
    fn ndk_home_comes_before_sdk_roots_and_roots_keep_order() {
        let mut env = environment();
        env.ndk_home = Some(PathBuf::from("/ndk"));
        let candidates = glslc_candidates(&env);
        assert_eq!(
            candidates,
            vec![
                PathBuf::from("/ndk/shader-tools/linux-x86_64/glslc"),
                PathBuf::from("/sdk-a/ndk/30.0.1/shader-tools/linux-x86_64/glslc"),
                PathBuf::from("/sdk-b/ndk/30.0.1/shader-tools/linux-x86_64/glslc"),
            ]
        );
        let only_second = |path: &Path| path.starts_with("/sdk-b");
        assert_eq!(
            resolve_glslc(&env, &only_second).unwrap(),
            PathBuf::from("/sdk-b/ndk/30.0.1/shader-tools/linux-x86_64/glslc")
        );
    }

    #[test]
    fn without_version_sdk_roots_are_not_guessed() {
        let mut env = environment();
        env.ndk_version = None;
        assert!(glslc_candidates(&env).is_empty());
        let error = resolve_glslc(&env, &|_| true).unwrap_err();
        assert!(matches!(error, ToolError::GlslcNotFound { ref tried } if tried.is_empty()));
    }

    #[test]
    fn not_found_message_lists_paths_and_explains_the_fix() {
        let env = environment();
        let message = resolve_glslc(&env, &|_| false).unwrap_err().to_string();
        assert!(
            message.contains("/sdk-a/ndk/30.0.1/shader-tools/linux-x86_64/glslc"),
            "{message}"
        );
        assert!(message.contains("FASTICAMERA_GLSLC"), "{message}");
        assert!(message.contains(NDK_VERSION_PROPERTY), "{message}");
    }

    #[test]
    fn spirv_val_must_live_next_to_glslc() {
        let glslc = Path::new("/ndk/shader-tools/linux-x86_64/glslc");
        let found = sibling_tool(glslc, "spirv-val", &|path| {
            path.ends_with("shader-tools/linux-x86_64/spirv-val")
        });
        assert_eq!(
            found.unwrap(),
            PathBuf::from("/ndk/shader-tools/linux-x86_64/spirv-val")
        );
        let error = sibling_tool(glslc, "spirv-val", &|_| false).unwrap_err();
        assert!(matches!(error, ToolError::SiblingMissing { ref tool, .. } if tool == "spirv-val"));
        assert!(error.to_string().contains("spirv-val"));
    }

    #[test]
    fn constant_names_are_derived_from_file_names() {
        assert_eq!(
            constant_name_for("fullscreen_triangle.vert").as_deref(),
            Some("FULLSCREEN_TRIANGLE_VERT")
        );
        assert_eq!(constant_name_for("sum-r16.comp").as_deref(), Some("SUM_R16_COMP"));
        assert_eq!(constant_name_for("readme.md"), None);
        assert_eq!(constant_name_for("noextension"), None);
        assert_eq!(constant_name_for(".vert"), None);
    }

    #[test]
    fn workspace_root_is_found_by_walking_up_to_gradle_properties() {
        let marker = |path: &Path| path == Path::new("/repo/android/gradle.properties");
        assert_eq!(
            find_workspace_root(Path::new("/repo/crates/isp-vulkan"), &marker),
            Some(PathBuf::from("/repo"))
        );
        assert_eq!(
            find_workspace_root(Path::new("/repo/crates/gpu-vulkan"), &marker),
            Some(PathBuf::from("/repo"))
        );
        assert_eq!(find_workspace_root(Path::new("/elsewhere/crate"), &marker), None);
    }

    #[test]
    fn real_workspace_root_of_this_crate_has_gradle_properties() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = find_workspace_root(manifest, &|path: &Path| path.is_file()).unwrap();
        assert!(root.join("Cargo.toml").is_file(), "{}", root.display());
        assert!(manifest.starts_with(&root));
    }

    #[test]
    fn host_tag_matches_operating_system() {
        assert_eq!(host_tag_for("linux"), "linux-x86_64");
        assert_eq!(host_tag_for("macos"), "darwin-x86_64");
        assert_eq!(host_tag_for("windows"), "windows-x86_64");
    }
}
