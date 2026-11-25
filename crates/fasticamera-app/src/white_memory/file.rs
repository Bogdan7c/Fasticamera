//! Файл памяти белой: чтение при старте (с карантином испорченного) и атомарная запись.
//!
//! Формат (`schema_version` 1):
//! ```json
//! {"schema_version":1,"kelvin":3200.0,"duv":0.004,"origin":"source","resume_rule":"keep_last"}
//! ```
//! Два независимых места: белая (`kelvin`, `duv`, `origin` — все три вместе или ни одного) и правило возобновления
//! (`resume_rule`: `take_from_source` или `keep_last`). Хватает любого одного: выбор правила возможен до первой
//! белой, а файл прежних версий хранит только белую. Частично заданная белая, документ без белой и без правила,
//! неизвестное поле, неизвестное происхождение или правило, файл новее приложения, пустой или не-JSON файл —
//! «не читается»:
//! белая берёт запасной свет, а сам файл не пропадает, а откладывается в `<имя>.corrupt-<unix-мс>`
//! (образец — `look-presets`), чтобы владелец мог его разобрать. Приложение при этом не падает.
//!
//! Запись: временный файл рядом → `write_all` → `sync_all` → `rename` поверх → сброс каталога. Прежний файл
//! при любом сбое остаётся целым.

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use camera_core::white::{RememberedOrigin, WhiteMemoryLoad, WhiteMemoryRecord, WhiteResumeRule};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::config::WhiteMemoryConfig;

/// Цель журнала памяти белой (общая с событиями белой: одним фильтром `fasticamera::white`).
pub(super) const LOG_TARGET: &str = "fasticamera::white";

/// Версия схемы, которую пишет и понимает это приложение.
const CURRENT_SCHEMA_VERSION: u32 = 1;
/// Имя поля с версией схемы (читается до разбора остальных полей).
const SCHEMA_VERSION_FIELD: &str = "schema_version";
/// Метка карантинного файла: `<имя файла>.corrupt-<unix-мс>`.
pub const QUARANTINE_FILE_MARKER: &str = ".corrupt-";

/// Происхождение в файле. Отдельный тип от `RememberedOrigin`: формат файла не должен меняться
/// вместе с внутренним типом движка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FileOrigin {
    Source,
    Manual,
}

/// Правило возобновления в файле. Отдельный тип от `WhiteResumeRule` по той же причине, что и `FileOrigin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FileResumeRule {
    TakeFromSource,
    KeepLast,
}

impl FileResumeRule {
    fn from_rule(rule: WhiteResumeRule) -> Self {
        match rule {
            WhiteResumeRule::TakeFromSource => Self::TakeFromSource,
            WhiteResumeRule::KeepLast => Self::KeepLast,
        }
    }

    fn into_rule(self) -> WhiteResumeRule {
        match self {
            Self::TakeFromSource => WhiteResumeRule::TakeFromSource,
            Self::KeepLast => WhiteResumeRule::KeepLast,
        }
    }
}

/// Всё, что файл помнит: белая и правило возобновления. Оба места независимы и необязательны по отдельности.
/// Владелец «последнего известного» содержимого — [`WhiteMemoryFile`]: запись одного места не стирает другое.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct FileContent {
    record: Option<WhiteMemoryRecord>,
    rule: Option<WhiteResumeRule>,
}

/// Документ файла. `deny_unknown_fields`: чужое поле — признак чужой или новой версии файла. Поля белой
/// необязательны только все вместе (см. [`Self::into_content`]); пустое место не пишется вовсе.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WhiteFileDocument {
    schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kelvin: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    duv: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    origin: Option<FileOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resume_rule: Option<FileResumeRule>,
}

impl WhiteFileDocument {
    fn from_content(content: &FileContent) -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            kelvin: content.record.map(|record| record.setting.temperature.kelvin()),
            duv: content.record.map(|record| record.setting.tint.duv()),
            origin: content.record.map(|record| match record.origin {
                RememberedOrigin::Source => FileOrigin::Source,
                RememberedOrigin::Manual => FileOrigin::Manual,
            }),
            resume_rule: content.rule.map(FileResumeRule::from_rule),
        }
    }

    /// Значения проверяются типами домена: нечисло и неположительная температура — «не читается». Белая задана
    /// целиком или не задана; документ без белой и без правила ничего не помнит и тоже «не читается».
    fn into_content(self) -> Result<FileContent, String> {
        let record = match (self.kelvin, self.duv, self.origin) {
            (Some(kelvin), Some(duv), Some(origin)) => {
                let temperature = ColorTemperature::from_kelvin(kelvin).map_err(|error| error.to_string())?;
                let tint = Tint::from_duv(duv).map_err(|error| error.to_string())?;
                Some(WhiteMemoryRecord {
                    setting: WhiteBalanceSetting { temperature, tint },
                    origin: match origin {
                        FileOrigin::Source => RememberedOrigin::Source,
                        FileOrigin::Manual => RememberedOrigin::Manual,
                    },
                })
            }
            (None, None, None) => None,
            (kelvin, duv, origin) => {
                let missing: Vec<&str> = [
                    ("kelvin", kelvin.is_none()),
                    ("duv", duv.is_none()),
                    ("origin", origin.is_none()),
                ]
                .into_iter()
                .filter_map(|(field, is_missing)| is_missing.then_some(field))
                .collect();
                return Err(format!("белая задана частично: нет {}", missing.join(", ")));
            }
        };
        let rule = self.resume_rule.map(FileResumeRule::into_rule);
        if record.is_none() && rule.is_none() {
            return Err("в файле нет ни белой, ни правила возобновления".to_owned());
        }
        Ok(FileContent { record, rule })
    }
}

/// Почему запись записать не удалось. Стадия видна в тексте: «нет каталога» и «диск полон» лечатся по-разному.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WhiteMemoryError {
    #[error("запись не сериализована: {0}")]
    Serialize(String),
    #[error("временный файл не создан: {0}")]
    CreateTemp(String),
    #[error("запись во временный файл не удалась: {0}")]
    Write(String),
    #[error("временный файл не сброшен на диск: {0}")]
    Sync(String),
    #[error("временный файл не переименован поверх основного: {0}")]
    Replace(String),
    /// Файл при старте не прочитался или не отложился в карантин: чужое не затираем.
    #[error("запись отключена: {0}")]
    SavesRefused(String),
}

/// Чем кончилась успешная запись.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteCommit {
    /// Файл заменён и каталог сброшен на диск.
    Durable,
    /// Файл заменён (данные не потеряны), но сброс каталога не подтверждён: при потере питания в этот миг
    /// мог бы вернуться прежний файл.
    DirectoryUnconfirmed(String),
}

/// Куда поток записи пишет память. Файл — [`WhiteMemoryFile`]; тесты подставляют заглушку.
///
/// Белая и правило возобновления — два независимых места одного хранилища: запись одного не стирает другое.
pub trait WhiteRecordSink: Send + 'static {
    /// Записать белую; правило хранилища остаётся прежним.
    fn write_record(&mut self, record: &WhiteMemoryRecord) -> Result<WriteCommit, WhiteMemoryError>;

    /// Записать правило возобновления; белая хранилища остаётся прежней.
    fn write_resume_rule(&mut self, rule: WhiteResumeRule) -> Result<WriteCommit, WhiteMemoryError>;

    /// Записать оба места за одну запись на диск (поток записи зовёт его, когда к пробуждению заняты оба). Здесь
    /// умолчание — два вызова подряд, для заглушек; файл переопределяет его одной записью: диску незачем
    /// получать два временных файла, переименования и сброса каталога там, где хватит одного.
    fn write_record_and_rule(
        &mut self,
        record: &WhiteMemoryRecord,
        rule: WhiteResumeRule,
    ) -> Result<WriteCommit, WhiteMemoryError> {
        self.write_record(record)?;
        self.write_resume_rule(rule)
    }
}

/// Файл памяти белой.
#[derive(Debug)]
pub struct WhiteMemoryFile {
    path: PathBuf,
    temp_path: PathBuf,
    /// Почему писать нельзя (файл не прочитался и не отложен): `Some` — записи отклоняются.
    saves_refused: Option<String>,
    /// Последнее известное содержимое: прочитанное при открытии и обновлённое каждой записью. Каждая запись
    /// переписывает файл целиком из него, поэтому белая не теряет правило, а правило — белую.
    known: FileContent,
}

/// Что вернуло открытие файла.
#[derive(Debug)]
pub struct OpenedWhiteFile {
    pub file: WhiteMemoryFile,
    /// Что файл помнит о белой.
    pub load: WhiteMemoryLoad,
    /// Правило возобновления, которое человек выбрал раньше (`None` — выбора в файле нет или файл не читается).
    pub resume_rule: Option<WhiteResumeRule>,
}

impl WhiteMemoryFile {
    /// Читает файл в папке приложения. Никогда не падает: любой исход — это [`WhiteMemoryLoad`] и запись в журнал.
    /// Конфиг уже проверен ([`WhiteMemoryConfig::validate`]).
    pub fn open(directory: &Path, config: &WhiteMemoryConfig) -> OpenedWhiteFile {
        let mut file = Self {
            path: directory.join(&config.file_name),
            temp_path: directory.join(config.temp_file_name()),
            saves_refused: None,
            known: FileContent::default(),
        };
        let load = file.read_and_judge();
        let resume_rule = file.known.rule;
        OpenedWhiteFile {
            file,
            load,
            resume_rule,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Почему запись отключена (`None` — пишет).
    pub fn saves_refused(&self) -> Option<&str> {
        self.saves_refused.as_deref()
    }

    /// Читает файл и приводит его к исходу; испорченный откладывает в карантин. Прочитанное содержимое
    /// становится «последним известным» (`known`).
    fn read_and_judge(&mut self) -> WhiteMemoryLoad {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                tracing::info!(target: LOG_TARGET, path = ?self.path, "файла памяти белой ещё нет: белая — запасной свет");
                return WhiteMemoryLoad::NothingRemembered;
            }
            Err(error) => {
                // Файл мог быть целым: ни читать, ни затирать его мы не можем, запись отключаем.
                let reason = format!("файл памяти белой не прочитан: {error}");
                tracing::warn!(target: LOG_TARGET, path = ?self.path, %reason, "память белой не читается, запись отключена");
                self.saves_refused = Some(reason.clone());
                return WhiteMemoryLoad::Unreadable { reason };
            }
        };
        match parse_content(&bytes) {
            Ok(content) => {
                tracing::info!(
                    target: LOG_TARGET,
                    path = ?self.path,
                    has_white = content.record.is_some(),
                    resume_rule = ?content.rule,
                    "память белой прочитана"
                );
                self.known = content;
                content
                    .record
                    .map_or(WhiteMemoryLoad::NothingRemembered, WhiteMemoryLoad::Remembered)
            }
            Err(problem) => self.quarantine(&problem),
        }
    }

    /// Убирает испорченный файл в `<имя>.corrupt-<unix-мс>`; не вышло — запись отключается (чужое не затираем).
    fn quarantine(&mut self, problem: &str) -> WhiteMemoryLoad {
        let target = self.free_quarantine_path();
        match fs::rename(&self.path, &target) {
            Ok(()) => {
                let reason = format!("{problem}; файл отложен в {}", target.display());
                tracing::warn!(target: LOG_TARGET, %reason, "файл памяти белой испорчен: отложен в карантин");
                WhiteMemoryLoad::Unreadable { reason }
            }
            Err(error) => {
                let reason = format!("{problem}; в карантин не отложен: {error}");
                tracing::warn!(target: LOG_TARGET, %reason, "файл памяти белой испорчен и не отложен: запись отключена");
                self.saves_refused = Some(reason.clone());
                WhiteMemoryLoad::Unreadable { reason }
            }
        }
    }

    /// Свободное имя карантинного файла: время в мс, при совпадении — числовой хвост.
    fn free_quarantine_path(&self) -> PathBuf {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis())
            // Часы раньше 1970 года: берём 0, имя всё равно уникализируется хвостом.
            .unwrap_or(0);
        let name = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let base = format!("{name}{QUARANTINE_FILE_MARKER}{now_ms}");
        let mut candidate = self.path.with_file_name(&base);
        let mut attempt = 1_u32;
        while candidate.exists() {
            candidate = self.path.with_file_name(format!("{base}-{attempt}"));
            attempt += 1;
        }
        candidate
    }

    /// Пишет временный файл и сбрасывает его на диск; при сбое временный файл убирается.
    fn write_temp_file(&self, bytes: &[u8]) -> Result<(), WhiteMemoryError> {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&self.temp_path)
            .map_err(|error| WhiteMemoryError::CreateTemp(error.to_string()))?;
        let written = file
            .write_all(bytes)
            .map_err(|error| WhiteMemoryError::Write(error.to_string()))
            .and_then(|()| {
                file.sync_all()
                    .map_err(|error| WhiteMemoryError::Sync(error.to_string()))
            });
        if written.is_err() {
            self.discard_temp_file();
        }
        written
    }

    /// Убирает временный файл после сбоя; сама неудача уборки не критична, но не молчит.
    fn discard_temp_file(&self) {
        if let Err(error) = fs::remove_file(&self.temp_path)
            && error.kind() != ErrorKind::NotFound
        {
            tracing::warn!(target: LOG_TARGET, %error, "не удалось убрать временный файл памяти белой");
        }
    }

    /// Сбрасывает каталог на диск, чтобы переименование пережило отключение питания (только unix).
    #[cfg(unix)]
    fn sync_directory(&self) -> Result<(), String> {
        let directory = self.path.parent().unwrap_or_else(|| Path::new("."));
        File::open(directory)
            .and_then(|handle| handle.sync_all())
            .map_err(|error| error.to_string())
    }

    #[cfg(not(unix))]
    fn sync_directory(&self) -> Result<(), String> {
        Ok(())
    }
}

impl WhiteMemoryFile {
    /// Переписывает файл целиком из последнего известного содержимого. `known` обновляют вызывающие ДО записи:
    /// даже если запись не удалась, следующая попытка (любого места) отдаст на диск самое свежее, что известно.
    fn persist_known(&mut self) -> Result<WriteCommit, WhiteMemoryError> {
        if let Some(reason) = &self.saves_refused {
            return Err(WhiteMemoryError::SavesRefused(reason.clone()));
        }
        let bytes = serde_json::to_vec_pretty(&WhiteFileDocument::from_content(&self.known))
            .map_err(|error| WhiteMemoryError::Serialize(error.to_string()))?;
        self.write_temp_file(&bytes)?;
        if let Err(error) = fs::rename(&self.temp_path, &self.path) {
            self.discard_temp_file();
            return Err(WhiteMemoryError::Replace(error.to_string()));
        }
        // Файл уже заменён: сбой сброса каталога — не ошибка записи, а отдельный исход.
        Ok(match self.sync_directory() {
            Ok(()) => WriteCommit::Durable,
            Err(failure) => WriteCommit::DirectoryUnconfirmed(failure),
        })
    }
}

impl WhiteRecordSink for WhiteMemoryFile {
    fn write_record(&mut self, record: &WhiteMemoryRecord) -> Result<WriteCommit, WhiteMemoryError> {
        self.known.record = Some(*record);
        self.persist_known()
    }

    fn write_resume_rule(&mut self, rule: WhiteResumeRule) -> Result<WriteCommit, WhiteMemoryError> {
        self.known.rule = Some(rule);
        self.persist_known()
    }

    fn write_record_and_rule(
        &mut self,
        record: &WhiteMemoryRecord,
        rule: WhiteResumeRule,
    ) -> Result<WriteCommit, WhiteMemoryError> {
        self.known = FileContent {
            record: Some(*record),
            rule: Some(rule),
        };
        self.persist_known()
    }
}

/// Разбирает байты файла: JSON → версия схемы → поля → значения. `Err` — человекочитаемая причина.
fn parse_content(bytes: &[u8]) -> Result<FileContent, String> {
    let document: Value = serde_json::from_slice(bytes).map_err(|error| format!("файл не JSON: {error}"))?;
    let found = document
        .get(SCHEMA_VERSION_FIELD)
        .and_then(Value::as_u64)
        .and_then(|version| u32::try_from(version).ok())
        .ok_or_else(|| format!("нет поля {SCHEMA_VERSION_FIELD} (целое число)"))?;
    if found > CURRENT_SCHEMA_VERSION {
        return Err(format!(
            "схема {found} новее поддерживаемой {CURRENT_SCHEMA_VERSION}: файл от более новой версии приложения"
        ));
    }
    if found < CURRENT_SCHEMA_VERSION {
        // Миграций пока нет: схемы 0 не существовало, такой файл чужой.
        return Err(format!(
            "схема {found} неизвестна (поддерживается {CURRENT_SCHEMA_VERSION})"
        ));
    }
    serde_json::from_value::<WhiteFileDocument>(document)
        .map_err(|error| format!("поля файла не подходят: {error}"))?
        .into_content()
}
