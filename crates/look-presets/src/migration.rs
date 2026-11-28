//! Миграции схемы файла пресетов: цепочка чистых функций `JSON → JSON` по номеру версии.
//!
//! История схемы:
//! - **v1** — первая версия: `LookSettings` без колёс цвета.
//! - **v2** (сессия 14i, решения 186, 191–193) — в каждые `LookSettings` добавлено обязательное поле
//!   `wheels` (колёса Lift/Gamma/Gain/Offset). Поле обязательное намеренно, без `#[serde(default)]`:
//!   «тихо» достроить недостающее serde не должен, это работа явного шага [`migrate_v1_to_v2`].
//!   Так файл версии 2 без `wheels` — это порча (карантин), а не тихая нейтраль.
//!
//! - **v3** (сессия 14b, решения 195, 196) — основа тона стала частью пресета: у каждого пользовательского
//!   пресета рядом с `settings` появилось обязательное поле `base`, а `edits` из «голых» значений ручек стал
//!   рецептом `{"base": …, "settings": …}`. Поле обязательное намеренно (как `wheels` в v2).
//!
//! - **v4** (сессия 14t, решения 339–343) — в каждые `LookSettings` добавлено обязательное поле `primaries`
//!   (12 полосок Lift/Gamma/Gain/Offset × R/G/B). Поле обязательное намеренно (как `wheels` в v2): достроить его
//!   умеет только явный шаг [`migrate_v3_to_v4`].
//!
//! Файл старше текущей версии без шага в таблице считается непереносимым и уходит в карантин
//! (решение 179); перед первой записью после миграции делается копия старого файла `.pre-v<N>`.

use serde_json::{Value, json};

/// Текущая версия схемы файла пресетов.
///
/// 14l схему не подняла: состав файла прежний, изменился смысл чисел — что значит край ±1, решает модель вида
/// (`look_model`), а не файл; пересчитывать сохранённые значения владелец не стал (решение 237).
pub const CURRENT_SCHEMA_VERSION: u32 = 4;

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

/// Продуктовая таблица миграций: v1 → v2 (колёса цвета), v2 → v3 (основа тона в пресете),
/// v3 → v4 (полоски по каналам).
pub const PRODUCT_MIGRATIONS: &[Migration] = &[
    Migration {
        from_version: 1,
        step: migrate_v1_to_v2,
    },
    Migration {
        from_version: 2,
        step: migrate_v2_to_v3,
    },
    Migration {
        from_version: 3,
        step: migrate_v3_to_v4,
    },
];

/// Версия, из которой идёт шаг [`migrate_v1_to_v2`] (для текста ошибок).
const V1: u32 = 1;

/// Версия, из которой идёт шаг [`migrate_v2_to_v3`] (для текста ошибок).
const V2: u32 = 2;

/// Версия, из которой идёт шаг [`migrate_v3_to_v4`] (для текста ошибок).
const V3: u32 = 3;

/// Имя нового поля с полосками по каналам внутри `LookSettings` (схема 4).
const PRIMARIES_FIELD: &str = "primaries";

/// Имя поля с основой тона в схеме 3.
const BASE_FIELD: &str = "base";

/// Имя поля со значениями ручек внутри рецепта и пресета.
const SETTINGS_FIELD: &str = "settings";

/// Основа, которую шаг v2 → v3 приписывает всем пресетам файла схемы 2, литералом в форме схемы 3.
///
/// Почему `"agx"`: пока писалась схема 2, основа выбиралась только конфигом рендера
/// (`render.viewfinder.tone.base`), и её умолчание было AgX (`ToneBaseChoice::Agx`, `#[default]`):
/// все виды, которые владелец сохранил в схеме 2, он видел именно на AgX. Литерал, а не живое умолчание:
/// шаг обязан всегда выдавать ровно схему 3, даже если умолчание конфига потом сменится (тот же принцип,
/// что у [`neutral_wheels_v2`]); тест сверяет литерал с живой сериализацией `ToneBaseModel::Agx`.
const BASE_FOR_SCHEMA_2_FILES: &str = "agx";

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
    step_failed_from(V1, reason)
}

/// Ошибка шага «из версии `from_version`» с причиной.
fn step_failed_from(from_version: u32, reason: impl Into<String>) -> MigrationError {
    MigrationError::StepFailed {
        from_version,
        reason: reason.into(),
    }
}

/// Шаг v2 → v3: каждому пользовательскому пресету добавляет `base` рядом с `settings`, а `edits`
/// (если там объект) оборачивает в рецепт `{"base": …, "settings": <прежние edits>}`.
/// `edits: null` («правок нет») остаётся как есть.
///
/// Ошибки (все `StepFailed` из версии 2): `user_presets` нет или не массив; пресет не объект; `base`
/// уже есть (в схеме 2 его быть не может, значит файл не схемы 2); у `edits` уже есть `base`.
fn migrate_v2_to_v3(mut document: Value) -> Result<Value, MigrationError> {
    let root = document.as_object_mut().ok_or(MigrationError::RootNotObject)?;

    let presets = root
        .get_mut("user_presets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| step_failed_from(V2, "поле user_presets отсутствует или не массив"))?;
    for (index, preset) in presets.iter_mut().enumerate() {
        let object = preset
            .as_object_mut()
            .ok_or_else(|| step_failed_from(V2, format!("пресет №{index} не объект")))?;
        if object.contains_key(BASE_FIELD) {
            return Err(step_failed_from(
                V2,
                format!("у пресета №{index} уже есть base: файл не версии 2"),
            ));
        }
        object.insert(BASE_FIELD.to_owned(), Value::from(BASE_FOR_SCHEMA_2_FILES));
    }

    // `edits: null` — правок нет, оборачивать нечего. Объект — прежние значения ручек: кладём их в рецепт.
    if let Some(edits) = root.get_mut("edits").filter(|edits| edits.is_object()) {
        if edits.get(BASE_FIELD).is_some() {
            return Err(step_failed_from(V2, "в edits уже есть base: файл не версии 2"));
        }
        let settings = edits.take();
        *edits = json!({ BASE_FIELD: BASE_FOR_SCHEMA_2_FILES, SETTINGS_FIELD: settings });
    }
    Ok(document)
}

/// Нейтральные полоски в форме СХЕМЫ ВЕРСИИ 4, записанные литералом: 4 зоны × 3 канала (`red`/`green`/`blue`).
///
/// Намеренно НЕ `serde_json::to_value(ChannelPrimaries::NEUTRAL)`: шаг обязан всегда выдавать ровно схему 4, даже
/// если словарь полосок потом изменится (тот же принцип, что у [`neutral_wheels_v2`]). Тест сверяет литерал с живой
/// нейтралью на сегодня.
fn neutral_primaries_v4() -> Value {
    let neutral_triple = || json!({"red": 0.0, "green": 0.0, "blue": 0.0});
    json!({
        "lift": neutral_triple(),
        "gamma": neutral_triple(),
        "gain": neutral_triple(),
        "offset": neutral_triple(),
    })
}

/// Шаг v3 → v4: вставляет нейтральные полоски в `settings` каждого пользовательского пресета и в
/// `edits.settings` (в схеме 3 `edits` — рецепт `{base, settings}` или `null`; `null` остаётся как есть).
///
/// Ошибки (все `StepFailed` из версии 3, различимы по тексту): `user_presets` нет или не массив; у пресета нет
/// `settings` или он не объект; у `edits` нет `settings` или он не объект; `primaries` уже есть (в схеме 3 его быть
/// не может, значит файл не схемы 3). Прежние значения не меняются.
fn migrate_v3_to_v4(mut document: Value) -> Result<Value, MigrationError> {
    let root = document.as_object_mut().ok_or(MigrationError::RootNotObject)?;

    let presets = root
        .get_mut("user_presets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| step_failed_from(V3, "поле user_presets отсутствует или не массив"))?;
    for (index, preset) in presets.iter_mut().enumerate() {
        let settings = preset
            .get_mut(SETTINGS_FIELD)
            .ok_or_else(|| step_failed_from(V3, format!("у пресета №{index} нет поля settings")))?;
        insert_neutral_primaries(settings, &format!("settings пресета №{index}"))?;
    }

    // `edits: null` — правок нет, вставлять некуда. Объект — рецепт `{base, settings}`: полоски кладём в его settings.
    if let Some(edits) = root.get_mut("edits").filter(|edits| edits.is_object()) {
        let settings = edits
            .get_mut(SETTINGS_FIELD)
            .ok_or_else(|| step_failed_from(V3, "у edits нет поля settings"))?;
        insert_neutral_primaries(settings, "edits.settings")?;
    }
    Ok(document)
}

/// Кладёт нейтральные полоски в объект `settings`; `place` — для текста ошибки.
fn insert_neutral_primaries(settings: &mut Value, place: &str) -> Result<(), MigrationError> {
    let object = settings
        .as_object_mut()
        .ok_or_else(|| step_failed_from(V3, format!("{place} не объект")))?;
    if object.contains_key(PRIMARIES_FIELD) {
        return Err(step_failed_from(
            V3,
            format!("в {place} уже есть primaries: файл не версии 3"),
        ));
    }
    object.insert(PRIMARIES_FIELD.to_owned(), neutral_primaries_v4());
    Ok(())
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

    /// Минимальный файл v2: один пресет и правки (колёса уже есть).
    fn minimal_v2() -> Value {
        json!({
            "schema_version": 2,
            "next_id": 2,
            "active": {"user": 1},
            "edits": {"contrast": -0.5, "bands": [], "wheels": neutral_wheels_v2()},
            "user_presets": [
                {"id": 1, "name": "A", "settings": {"contrast": 0.3, "bands": [], "wheels": neutral_wheels_v2()}},
                {"id": 2, "name": "B", "settings": {"contrast": 0.6, "bands": [], "wheels": neutral_wheels_v2()}}
            ]
        })
    }

    /// Минимальный файл v3: рецепты `{base, settings}` без полосок.
    fn minimal_v3() -> Value {
        json!({
            "schema_version": 3,
            "next_id": 2,
            "active": {"user": 1},
            "edits": {"base": "aces", "settings": {"contrast": -0.5, "bands": [], "wheels": neutral_wheels_v2()}},
            "user_presets": [
                {"id": 1, "name": "A", "base": "agx",
                 "settings": {"contrast": 0.3, "bands": [], "wheels": neutral_wheels_v2()}},
                {"id": 2, "name": "B", "base": "filmic",
                 "settings": {"contrast": 0.6, "bands": [], "wheels": neutral_wheels_v2()}}
            ]
        })
    }

    fn step_reason_v3(result: Result<Value, MigrationError>) -> String {
        match result {
            Err(MigrationError::StepFailed {
                from_version: 3,
                reason,
            }) => reason,
            other => panic!("ждали StepFailed из v3, получили {other:?}"),
        }
    }

    #[test]
    fn frozen_v4_neutral_primaries_literal_matches_live_neutral_today() {
        let live = serde_json::to_value(look_contract::ChannelPrimaries::NEUTRAL).unwrap();
        assert_eq!(neutral_primaries_v4(), live);
    }

    #[test]
    fn v3_step_adds_primaries_to_every_preset_and_to_edits_settings() {
        let migrated = migrate_v3_to_v4(minimal_v3()).unwrap();
        for index in 0..2 {
            assert_eq!(
                migrated["user_presets"][index]["settings"]["primaries"],
                neutral_primaries_v4()
            );
        }
        assert_eq!(migrated["edits"]["settings"]["primaries"], neutral_primaries_v4());

        let mut no_edits = minimal_v3();
        no_edits["edits"] = Value::Null;
        assert_eq!(migrate_v3_to_v4(no_edits).unwrap()["edits"], Value::Null);
    }

    #[test]
    fn v3_step_changes_nothing_except_inserted_primaries() {
        let mut migrated = migrate_v3_to_v4(minimal_v3()).unwrap();
        migrated["edits"]["settings"]
            .as_object_mut()
            .unwrap()
            .remove("primaries");
        for preset in migrated["user_presets"].as_array_mut().unwrap() {
            preset["settings"].as_object_mut().unwrap().remove("primaries");
        }
        assert_eq!(migrated, minimal_v3());
    }

    #[test]
    fn v3_step_failures_are_distinguishable() {
        let no_presets = step_reason_v3(migrate_v3_to_v4(json!({"edits": null})));
        assert!(no_presets.contains("user_presets"), "{no_presets}");
        let settings_missing = step_reason_v3(migrate_v3_to_v4(json!({"edits": null, "user_presets": [{"id": 1}]})));
        assert!(settings_missing.contains("нет поля settings"), "{settings_missing}");
        let settings_not_object = step_reason_v3(migrate_v3_to_v4(
            json!({"edits": null, "user_presets": [{"id": 1, "settings": 5}]}),
        ));
        assert!(settings_not_object.contains("не объект"), "{settings_not_object}");
        let mut already = minimal_v3();
        already["user_presets"][1]["settings"]["primaries"] = json!({});
        assert!(step_reason_v3(migrate_v3_to_v4(already)).contains("пресета №1 уже есть primaries"));
        let mut edits_already = minimal_v3();
        edits_already["edits"]["settings"]["primaries"] = json!({});
        assert!(step_reason_v3(migrate_v3_to_v4(edits_already)).contains("edits.settings уже есть primaries"));
        let edits_without_settings =
            step_reason_v3(migrate_v3_to_v4(json!({"edits": {"base": "agx"}, "user_presets": []})));
        assert!(
            edits_without_settings.contains("у edits нет поля settings"),
            "{edits_without_settings}"
        );
        assert_eq!(migrate_v3_to_v4(json!([])), Err(MigrationError::RootNotObject));
    }

    fn step_reason_v2(result: Result<Value, MigrationError>) -> String {
        match result {
            Err(MigrationError::StepFailed {
                from_version: 2,
                reason,
            }) => reason,
            other => panic!("ждали StepFailed из v2, получили {other:?}"),
        }
    }

    #[test]
    fn product_chain_reaches_current_version_and_stamps_it() {
        assert_eq!(CURRENT_SCHEMA_VERSION, 4);
        let migrated = migrate_to_current(minimal_v1(), 1, PRODUCT_MIGRATIONS).unwrap();
        assert_eq!(migrated["schema_version"], 4);
        assert_eq!(
            migrated["user_presets"][0]["settings"]["primaries"],
            neutral_primaries_v4()
        );
        assert_eq!(migrated["edits"]["settings"]["primaries"], neutral_primaries_v4());
        assert_eq!(migrated["user_presets"][0]["settings"]["wheels"], neutral_wheels_v2());
        assert_eq!(migrated["user_presets"][0]["base"], "agx");
        assert_eq!(migrated["edits"]["base"], "agx");
        assert_eq!(migrated["edits"]["settings"]["wheels"], neutral_wheels_v2());
        let from_v2 = migrate_to_current(minimal_v2(), 2, PRODUCT_MIGRATIONS).unwrap();
        assert_eq!(from_v2["schema_version"], 4);
        let from_v3 = migrate_to_current(minimal_v3(), 3, PRODUCT_MIGRATIONS).unwrap();
        assert_eq!(from_v3["schema_version"], 4);
    }

    #[test]
    fn frozen_base_literal_matches_live_agx_serialization_today() {
        let live = serde_json::to_value(look_contract::ToneBaseModel::Agx).unwrap();
        assert_eq!(Value::from(BASE_FOR_SCHEMA_2_FILES), live);
    }

    #[test]
    fn v2_step_adds_base_next_to_settings_and_wraps_edits() {
        let migrated = migrate_v2_to_v3(minimal_v2()).unwrap();
        for index in 0..2 {
            assert_eq!(migrated["user_presets"][index]["base"], "agx");
            assert_eq!(
                migrated["user_presets"][index]["settings"],
                minimal_v2()["user_presets"][index]["settings"],
                "ручки пресета не тронуты"
            );
        }
        assert_eq!(migrated["edits"]["base"], "agx");
        assert_eq!(migrated["edits"]["settings"], minimal_v2()["edits"]);

        let mut no_edits = minimal_v2();
        no_edits["edits"] = Value::Null;
        assert_eq!(migrate_v2_to_v3(no_edits).unwrap()["edits"], Value::Null);
    }

    #[test]
    fn v2_step_changes_nothing_except_base_and_edits_wrapper() {
        let mut migrated = migrate_v2_to_v3(minimal_v2()).unwrap();
        for preset in migrated["user_presets"].as_array_mut().unwrap() {
            preset.as_object_mut().unwrap().remove("base");
        }
        let wrapped = migrated["edits"].take();
        migrated["edits"] = wrapped["settings"].clone();
        assert_eq!(migrated, minimal_v2());
    }

    #[test]
    fn v2_step_failures_are_distinguishable() {
        let no_presets = step_reason_v2(migrate_v2_to_v3(json!({"edits": null})));
        assert!(no_presets.contains("user_presets"), "{no_presets}");
        let not_object = step_reason_v2(migrate_v2_to_v3(json!({"edits": null, "user_presets": [5]})));
        assert!(not_object.contains("не объект"), "{not_object}");
        let mut already = minimal_v2();
        already["user_presets"][1]["base"] = json!("aces");
        assert!(step_reason_v2(migrate_v2_to_v3(already)).contains("пресета №1 уже есть base"));
        let mut edits_already = minimal_v2();
        edits_already["edits"]["base"] = json!("aces");
        assert!(step_reason_v2(migrate_v2_to_v3(edits_already)).contains("в edits уже есть base"));
        assert_eq!(migrate_v2_to_v3(json!([])), Err(MigrationError::RootNotObject));
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
        let document = json!({"schema_version": 4, "a": 1});
        assert_eq!(
            migrate_to_current(document.clone(), 4, PRODUCT_MIGRATIONS),
            Ok(document)
        );
    }
}
