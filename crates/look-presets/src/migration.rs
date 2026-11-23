//! Миграции схемы файла пресетов: цепочка чистых функций `JSON → JSON` по номеру версии.
//!
//! История схемы:
//! - **v1** — первая версия: `LookSettings` без колёс цвета.
//! - **v2** (сессия 14i, решения 186, 191–193) — в каждые `LookSettings` добавлено обязательное поле
//!   `wheels` (колёса Lift/Gamma/Gain/Offset). Поле обязательное намеренно, без `#[serde(default)]`:
//!   «тихо» достроить недостающее serde не должен, это работа явного шага [`migrate_v1_to_v2`].
//!   Так файл версии 2 без `wheels` — это порча (карантин), а не тихая нейтраль.
//!
//! Файл старше текущей версии без шага в таблице считается непереносимым и уходит в карантин
//! (решение 179); перед первой записью после миграции делается копия старого файла `.pre-v<N>`.

use serde_json::{Value, json};

/// Текущая версия схемы файла пресетов.
pub const CURRENT_SCHEMA_VERSION: u32 = 2;

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

/// Продуктовая таблица миграций: пока один шаг, v1 → v2 (колёса цвета).
pub const PRODUCT_MIGRATIONS: &[Migration] = &[Migration {
    from_version: 1,
    step: migrate_v1_to_v2,
}];

/// Версия, из которой идёт шаг [`migrate_v1_to_v2`] (для текста ошибок).
const V1: u32 = 1;

/// Имя нового поля с колёсами внутри `LookSettings`.
const WHEELS_FIELD: &str = "wheels";

/// Нейтральные колёса в форме СХЕМЫ ВЕРСИИ 2, записанные литералом.
///
/// Намеренно НЕ `serde_json::to_value(ColorWheels::NEUTRAL)`: шаг миграции обязан всегда выдавать
/// именно v2, даже если словарь колёс позже изменится (появится v3 со своим шагом). Если брать
/// значение из живого типа, старый шаг молча начал бы выдавать форму новой схемы, а следующий шаг
/// получил бы не то, что ждёт. Поэтому форма заморожена здесь, и тест сверяет её с живой нейтралью
/// на сегодня.
fn neutral_wheels_v2() -> Value {
    let neutral_wheel = || json!({"push": [0.0, 0.0], "ring": 0.0});
    json!({
        "lift": neutral_wheel(),
        "gamma": neutral_wheel(),
        "gain": neutral_wheel(),
        "offset": neutral_wheel(),
    })
}

/// Шаг v1 → v2: вставляет нейтральные колёса в `settings` каждого пользовательского пресета и в
/// `edits` (если там объект; `edits: null` — «правок нет» — остаётся как есть).
///
/// Ошибки (все `StepFailed`, различимы по тексту причины): `user_presets` нет или не массив;
/// `settings` пресета не объект; `wheels` уже есть (в v1 его быть не может, значит файл не v1).
fn migrate_v1_to_v2(mut document: Value) -> Result<Value, MigrationError> {
    let root = document.as_object_mut().ok_or(MigrationError::RootNotObject)?;

    let presets = root
        .get_mut("user_presets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| step_failed("поле user_presets отсутствует или не массив"))?;
    for (index, preset) in presets.iter_mut().enumerate() {
        let settings = preset
            .get_mut("settings")
            .ok_or_else(|| step_failed(format!("у пресета №{index} нет поля settings")))?;
        insert_neutral_wheels(settings, &format!("settings пресета №{index}"))?;
    }

    // `edits: null` — правок нет, вставлять некуда. Объект — правки поверх пресета.
    if let Some(edits) = root.get_mut("edits").filter(|edits| edits.is_object()) {
        insert_neutral_wheels(edits, "edits")?;
    }
    Ok(document)
}

/// Кладёт нейтральные колёса в объект `settings`; `place` — для текста ошибки.
fn insert_neutral_wheels(settings: &mut Value, place: &str) -> Result<(), MigrationError> {
    let object = settings
        .as_object_mut()
        .ok_or_else(|| step_failed(format!("{place} не объект")))?;
    if object.contains_key(WHEELS_FIELD) {
        return Err(step_failed(format!("в {place} уже есть wheels: файл не версии 1")));
    }
    object.insert(WHEELS_FIELD.to_owned(), neutral_wheels_v2());
    Ok(())
}

/// Ошибка шага v1 → v2 с причиной.
fn step_failed(reason: impl Into<String>) -> MigrationError {
    MigrationError::StepFailed {
        from_version: V1,
        reason: reason.into(),
    }
}

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

    /// Минимальный файл v1: один пресет и правки.
    fn minimal_v1() -> Value {
        json!({
            "schema_version": 1,
            "next_id": 2,
            "active": {"user": 1},
            "edits": {"contrast": -0.5, "bands": []},
            "user_presets": [
                {"id": 1, "name": "A", "settings": {"contrast": 0.3, "bands": []}},
                {"id": 2, "name": "B", "settings": {"contrast": 0.6, "bands": []}}
            ]
        })
    }

    fn step_reason(result: Result<Value, MigrationError>) -> String {
        match result {
            Err(MigrationError::StepFailed {
                from_version: 1,
                reason,
            }) => reason,
            other => panic!("ждали StepFailed из v1, получили {other:?}"),
        }
    }

    #[test]
    fn product_chain_reaches_current_version_and_stamps_it() {
        assert_eq!(CURRENT_SCHEMA_VERSION, 2);
        let migrated = migrate_to_current(minimal_v1(), 1, PRODUCT_MIGRATIONS).unwrap();
        assert_eq!(migrated["schema_version"], 2);
        assert_eq!(migrated["user_presets"][0]["settings"]["wheels"], neutral_wheels_v2());
    }

    #[test]
    fn frozen_v2_neutral_literal_matches_live_neutral_today() {
        let live = serde_json::to_value(look_contract::ColorWheels::NEUTRAL).unwrap();
        assert_eq!(neutral_wheels_v2(), live);
    }

    #[test]
    fn step_adds_wheels_to_every_preset_and_to_edits_object_only() {
        let migrated = migrate_v1_to_v2(minimal_v1()).unwrap();
        let wheels = neutral_wheels_v2();
        assert_eq!(migrated["user_presets"][0]["settings"]["wheels"], wheels);
        assert_eq!(migrated["user_presets"][1]["settings"]["wheels"], wheels);
        assert_eq!(migrated["edits"]["wheels"], wheels);

        let with_null_edits = migrate_v1_to_v2(json!({"edits": null, "user_presets": []})).unwrap();
        assert_eq!(with_null_edits, json!({"edits": null, "user_presets": []}));
    }

    #[test]
    fn step_changes_nothing_except_inserted_wheels() {
        let mut migrated = migrate_v1_to_v2(minimal_v1()).unwrap();
        // Убираем вставленные ключи: остаток обязан совпасть с исходным файлом.
        migrated["edits"].as_object_mut().unwrap().remove("wheels");
        for preset in migrated["user_presets"].as_array_mut().unwrap() {
            preset["settings"].as_object_mut().unwrap().remove("wheels");
        }
        assert_eq!(migrated, minimal_v1());
    }

    #[test]
    fn step_failures_are_distinguishable() {
        let no_presets = step_reason(migrate_v1_to_v2(json!({"edits": null})));
        assert!(no_presets.contains("user_presets"), "{no_presets}");
        let presets_not_array = step_reason(migrate_v1_to_v2(json!({"user_presets": {}})));
        assert!(presets_not_array.contains("user_presets"), "{presets_not_array}");

        let settings_not_object = step_reason(migrate_v1_to_v2(
            json!({"edits": null, "user_presets": [{"id": 1, "settings": 5}]}),
        ));
        assert!(settings_not_object.contains("не объект"), "{settings_not_object}");
        let settings_missing = step_reason(migrate_v1_to_v2(json!({"edits": null, "user_presets": [{"id": 1}]})));
        assert!(settings_missing.contains("нет поля settings"), "{settings_missing}");

        let mut already = minimal_v1();
        already["user_presets"][0]["settings"]["wheels"] = json!({});
        let already_reason = step_reason(migrate_v1_to_v2(already));
        assert!(already_reason.contains("уже есть wheels"), "{already_reason}");
        let mut edits_already = minimal_v1();
        edits_already["edits"]["wheels"] = json!({});
        assert!(step_reason(migrate_v1_to_v2(edits_already)).contains("edits"));

        assert_eq!(migrate_v1_to_v2(json!([])), Err(MigrationError::RootNotObject));
    }

    #[test]
    fn test_table_migrates_v0_to_v1_and_stamps_version() {
        let table = [Migration {
            from_version: 0,
            step: rename_preset_to_active,
        }];
        let migrated = migrate(json!({"schema_version": 0, "preset": "x"}), 0, 1, &table).unwrap();
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
        let document = json!({"schema_version": 2, "a": 1});
        assert_eq!(
            migrate_to_current(document.clone(), 2, PRODUCT_MIGRATIONS),
            Ok(document)
        );
    }
}
