//! Папка приложения (`Context.getFilesDir()`): где живут пресеты вида и другие файлы владельца.
//!
//! Kotlin передаёт путь третьим аргументом `initialize` (как имя пакета, решение 106). Здесь он проверяется ОДИН
//! раз при старте: путь абсолютный, каталог существует. Дальше по приложению ходит уже проверенный тип, а не
//! голая строка, поэтому «папки нет» видна на старте различимой ошибкой, а не загадочным сбоем первой записи.
//!
//! Тип нейтральный (без JNI), поэтому проверка тестируется на ПК.

use std::path::{Path, PathBuf};

/// Проверенная папка приложения.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDirectory(PathBuf);

/// Почему путь не годится как папка приложения. Варианты различимы: каждый лечится по-своему.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AppDirectoryError {
    /// Kotlin прислал пустую строку.
    #[error("путь к папке приложения пустой")]
    Empty,
    /// Путь относительный: от какой папки его считать, неизвестно.
    #[error("путь к папке приложения не абсолютный: {path}")]
    NotAbsolute {
        /// Что прислали.
        path: String,
    },
    /// Каталога нет на диске.
    #[error("папки приложения нет на диске: {path}")]
    Missing {
        /// Что прислали.
        path: String,
    },
    /// Узнать, что лежит по пути, не удалось (нет прав на родителя, сбой ввода-вывода): это НЕ «папки нет»,
    /// лечится иначе (права), поэтому причина ОС не теряется.
    #[error("нет доступа к папке приложения {path}: {reason}")]
    Inaccessible {
        /// Что прислали.
        path: String,
        /// Текст ошибки ОС.
        reason: String,
    },
    /// Путь существует, но это не каталог.
    #[error("путь к папке приложения указывает не на каталог: {path}")]
    NotADirectory {
        /// Что прислали.
        path: String,
    },
}

impl AppDirectory {
    /// Проверяет строку из Kotlin (`filesDir.absolutePath`).
    pub fn new(raw: String) -> Result<Self, AppDirectoryError> {
        if raw.is_empty() {
            return Err(AppDirectoryError::Empty);
        }
        let path = PathBuf::from(&raw);
        if !path.is_absolute() {
            return Err(AppDirectoryError::NotAbsolute { path: raw });
        }
        // `metadata` различает «нет» (NotFound), «не каталог» и сбой доступа: последний не сливаем с «нет».
        match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_dir() => Ok(Self(path)),
            Ok(_) => Err(AppDirectoryError::NotADirectory { path: raw }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(AppDirectoryError::Missing { path: raw }),
            Err(error) => Err(AppDirectoryError::Inaccessible {
                path: raw,
                reason: error.to_string(),
            }),
        }
    }

    /// Путь к каталогу.
    pub fn path(&self) -> &Path {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, reason = "тест")]

    use super::*;

    fn unique_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fasticamera-app-dir-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_existing_absolute_directory_is_accepted_as_is() {
        let dir = unique_dir("ok");
        let accepted = AppDirectory::new(dir.to_string_lossy().into_owned()).expect("каталог годится");
        assert_eq!(accepted.path(), dir.as_path());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_empty_path_is_named_empty() {
        assert_eq!(AppDirectory::new(String::new()), Err(AppDirectoryError::Empty));
    }

    #[test]
    fn a_relative_path_is_named_not_absolute() {
        assert_eq!(
            AppDirectory::new("files".to_owned()),
            Err(AppDirectoryError::NotAbsolute {
                path: "files".to_owned()
            })
        );
    }

    #[test]
    fn a_missing_directory_is_named_missing() {
        let path = unique_dir("missing").join("nope");
        let raw = path.to_string_lossy().into_owned();
        assert_eq!(
            AppDirectory::new(raw.clone()),
            Err(AppDirectoryError::Missing { path: raw })
        );
    }

    #[test]
    fn a_file_is_named_not_a_directory() {
        let dir = unique_dir("file");
        let file = dir.join("plain.txt");
        std::fs::write(&file, b"x").unwrap();
        let raw = file.to_string_lossy().into_owned();
        assert_eq!(
            AppDirectory::new(raw.clone()),
            Err(AppDirectoryError::NotADirectory { path: raw })
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Родитель без прав на обход: `metadata` отвечает PermissionDenied, а не NotFound.
    #[cfg(unix)]
    #[test]
    fn a_permission_failure_is_named_inaccessible_not_missing() {
        use std::os::unix::fs::PermissionsExt;
        let parent = unique_dir("denied");
        let child = parent.join("files");
        std::fs::create_dir(&child).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o000)).unwrap();
        let result = AppDirectory::new(child.to_string_lossy().into_owned());
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        if result.is_ok() {
            std::fs::remove_dir_all(parent).unwrap();
            return; // права не действуют (root): сбой не воспроизвести
        }
        let error = result.unwrap_err();
        std::fs::remove_dir_all(parent).unwrap();
        assert!(
            matches!(&error, AppDirectoryError::Inaccessible { reason, .. } if !reason.is_empty()),
            "{error:?}"
        );
    }
}
