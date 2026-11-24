//! Тесты файла кэша пайплайнов: настоящая файловая система во временной папке.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "тест")]

use super::*;

fn temp_directory(tag: &str) -> (PathBuf, AppDirectory) {
    let path = std::env::temp_dir().join(format!("fasticamera-pcache-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    let directory = AppDirectory::new(path.to_string_lossy().into_owned()).unwrap();
    (path, directory)
}

#[test]
fn a_first_launch_without_a_file_is_not_an_error() {
    let (path, directory) = temp_directory("first");
    let file = PipelineCacheFile::in_app_directory(&directory);
    assert_eq!(file.load().unwrap(), None);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn saved_bytes_come_back_exactly_and_a_second_save_replaces_them_without_leftovers() {
    let (path, directory) = temp_directory("roundtrip");
    let file = PipelineCacheFile::in_app_directory(&directory);
    file.save(&[1, 2, 3, 4]).unwrap();
    assert_eq!(file.load().unwrap(), Some(vec![1, 2, 3, 4]));
    file.save(&[9; 100]).unwrap();
    assert_eq!(file.load().unwrap(), Some(vec![9; 100]));
    assert!(
        !path.join(TEMP_FILE_NAME).exists(),
        "временный файл переименован, а не оставлен"
    );
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn a_leftover_temp_file_from_a_killed_save_does_not_replace_the_good_file() {
    let (path, directory) = temp_directory("killed");
    let file = PipelineCacheFile::in_app_directory(&directory);
    file.save(&[7; 10]).unwrap();
    // Процесс убит посреди записи: обрезок остался под временным именем.
    std::fs::write(path.join(TEMP_FILE_NAME), [0_u8; 3]).unwrap();
    assert_eq!(file.load().unwrap(), Some(vec![7; 10]), "читается целый основной файл");
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn a_read_failure_and_a_write_failure_are_named_with_the_place() {
    let (path, directory) = temp_directory("errors");
    // Каталог на месте файла: чтение даёт ошибку, отличную от «нет файла», запись переименованием тоже не проходит.
    std::fs::create_dir_all(path.join(CACHE_FILE_NAME)).unwrap();
    let file = PipelineCacheFile::in_app_directory(&directory);
    let read = file.load().unwrap_err();
    assert!(matches!(read, PipelineCacheStoreError::Read { .. }), "{read}");
    assert!(read.to_string().contains(CACHE_FILE_NAME), "{read}");
    let write = file.save(&[1]).unwrap_err();
    assert!(matches!(write, PipelineCacheStoreError::Write { .. }), "{write}");
    std::fs::remove_dir_all(path).unwrap();
}
