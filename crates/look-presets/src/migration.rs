//! Миграции схемы файла пресетов: цепочка чистых функций `JSON → JSON` по номеру версии.
//!
//! ЧЕСТНО: в продукте версия схемы пока 1, поэтому таблица [`PRODUCT_MIGRATIONS`] ПУСТА и
//! ни одного настоящего шага нет. Механизм проверен тестовой таблицей (v0 → v1, цепочка
//! v1 → v3); первый настоящий шаг появится вместе с версией 2. Пока шагов нет, файл старше
//! текущей версии считается непереносимым и уходит в карантин (решение 179).

use serde_json::Value;

/// Текущая версия схемы файла пресетов.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// Имя поля с версией в корне JSON.
pub(crate) const SCHEMA_VERSION_FIELD: &str = "schema_version";

/// Ошибка миграции.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MigrationError {
    /// В таблице нет шага «из этой версии»: файл нельзя довести до текущей схемы.
    #[error("нет шага миграции из версии {from_version}")]
    NoStep {
        /// Версия, из которой шага нет.
        from_version: u32,
    },
    /// Шаг сказал, что содержимое не переносится.
    #[error("шаг миграции из версии {from_version} не выполнен: {reason}")]
    StepFailed {
        /// Версия, из которой шли.
        from_version: u32,
        /// Причина от шага.
        reason: String,
    },
    /// Корень документа не объект JSON: версию некуда записать.
    #[error("корень файла пресетов не объект JSON")]
    RootNotObject,
}

/// Один шаг: «версия N → версия N+1». Функция чистая: без ввода-вывода и часов.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    /// Версия, которую шаг умеет читать.
    pub from_version: u32,
    /// Преобразование документа. Версию в документе обновляет сам прогон, не шаг.
    pub step: fn(Value) -> Result<Value, MigrationError>,
}

/// Продуктовая таблица миграций: пока пуста (схема версии 1 — первая).
pub const PRODUCT_MIGRATIONS: &[Migration] = &[];

/// Доводит документ версии `found_version` до [`CURRENT_SCHEMA_VERSION`].
pub fn migrate_to_current(
    document: Value,
    found_version: u32,
    migrations: &[Migration],
) -> Result<Value, MigrationError> {
    migrate(document, found_version, CURRENT_SCHEMA_VERSION, migrations)
}

/// Прогоняет шаги от `from_version` до `target_version` включительно по цепочке.
pub(crate) fn migrate(
    mut document: Value,
    from_version: u32,
    target_version: u32,
    migrations: &[Migration],
) -> Result<Value, MigrationError> {
    let mut version = from_version;
    while version < target_version {
        let migration = migrations
            .iter()
            .find(|migration| migration.from_version == version)
            .ok_or(MigrationError::NoStep { from_version: version })?;
        document = (migration.step)(document)?;
        version += 1;
        let object = document.as_object_mut().ok_or(MigrationError::RootNotObject)?;
        object.insert(SCHEMA_VERSION_FIELD.to_owned(), Value::from(version));
    }
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rename_preset_to_active(mut document: Value) -> Result<Value, MigrationError> {
        let object = document.as_object_mut().ok_or(MigrationError::RootNotObject)?;
        let old = object.remove("preset").ok_or(MigrationError::StepFailed {
            from_version: 0,
            reason: "нет поля preset".into(),
        })?;
        object.insert("active".into(), old);
        Ok(document)
    }

    fn add_marker(mut document: Value) -> Result<Value, MigrationError> {
        let object = document.as_object_mut().ok_or(MigrationError::RootNotObject)?;
        object.insert("marker".into(), json!("from-1"));
        Ok(document)
    }

    fn add_second_marker(mut document: Value) -> Result<Value, MigrationError> {
        let object = document.as_object_mut().ok_or(MigrationError::RootNotObject)?;
        object.insert("second".into(), json!("from-2"));
        Ok(document)
    }

    #[test]
    fn product_table_is_empty_for_schema_one() {
        assert_eq!(CURRENT_SCHEMA_VERSION, 1);
        assert!(PRODUCT_MIGRATIONS.is_empty());
    }

    #[test]
    fn test_table_migrates_v0_to_v1_and_stamps_version() {
        let table = [Migration {
            from_version: 0,
            step: rename_preset_to_active,
        }];
        let migrated = migrate_to_current(json!({"schema_version": 0, "preset": "x"}), 0, &table).unwrap();
        assert_eq!(migrated, json!({"schema_version": 1, "active": "x"}));
    }

    #[test]
    fn chain_runs_steps_in_order() {
        let table = [
            Migration {
                from_version: 2,
                step: add_second_marker,
            },
            Migration {
                from_version: 1,
                step: add_marker,
            },
        ];
        let migrated = migrate(json!({"schema_version": 1}), 1, 3, &table).unwrap();
        assert_eq!(
            migrated,
            json!({"schema_version": 3, "marker": "from-1", "second": "from-2"})
        );
    }

    #[test]
    fn missing_step_and_failing_step_are_distinct_errors() {
        assert_eq!(
            migrate_to_current(json!({}), 0, PRODUCT_MIGRATIONS),
            Err(MigrationError::NoStep { from_version: 0 })
        );
        let table = [Migration {
            from_version: 0,
            step: rename_preset_to_active,
        }];
        let failed = migrate_to_current(json!({"unrelated": 1}), 0, &table);
        assert!(matches!(
            failed,
            Err(MigrationError::StepFailed { from_version: 0, .. })
        ));
    }

    #[test]
    fn current_version_needs_no_steps() {
        let document = json!({"schema_version": 1, "a": 1});
        assert_eq!(
            migrate_to_current(document.clone(), 1, PRODUCT_MIGRATIONS),
            Ok(document)
        );
    }
}
