//! Поиск инструментов сборки шейдеров (glslc, spirv-val из NDK) — чистая логика.
//!
//! Файл подключается дважды: из `build.rs` (`#[path]`) и из `src/shaders.rs` под `cfg(test)`,
//! чтобы логику поиска можно было тестировать обычным `cargo test`. Поэтому здесь только `std`
//! и никаких обращений к окружению процесса: всё приходит через [`ToolchainEnvironment`].

use std::fmt;
use std::path::{Path, PathBuf};

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
    fn host_tag_matches_operating_system() {
        assert_eq!(host_tag_for("linux"), "linux-x86_64");
        assert_eq!(host_tag_for("macos"), "darwin-x86_64");
        assert_eq!(host_tag_for("windows"), "windows-x86_64");
    }
}
