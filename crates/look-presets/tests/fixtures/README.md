# Фикстуры файла пресетов

`schema_v1_with_edits.json` и `schema_v1_no_edits.json` — настоящие файлы схемы 1, записанные кодом
`look-presets` на коммите `7f38ace` (до колёс цвета 14i), а не набранные руками. После 14i код больше
не умеет писать схему 1, поэтому файлы заморожены и служат входом теста миграции v1 → v2.

Как получены (временный тест, удалён сразу после генерации):

1. `PresetFileStore::open(<временная папка>, &PresetStoreConfig::default(), PRODUCT_MIGRATIONS)`.
2. `with_edits`: `SetLook` (контраст +0,3, теплота −0,25, полоса `sky` оттенок +0,4) →
   `SaveAsNewPreset("Evening")` → `SetLook` (контраст −0,5) → `save`.
3. `no_edits`: `SetLook` (контраст +0,6) → `SaveAsNewPreset("Punchy")` → `save` (`edits: null`).

Файлы не править: любая правка делает их «не настоящими» файлами старой версии.

## Схема 2

`schema_v2_with_edits.json` и `schema_v2_no_edits.json` — настоящие файлы схемы 2, записанные кодом `look-presets`
на коммите `dfe5162` (до основы тона в пресете, сессия 14b), а не набранные руками. Сохранённый вид с колёсами
цвета не нейтральный, чтобы миграция 2 → 3 доказывала, что ручки и колёса не тронуты. После 14b код пишет только
схему 3: файлы заморожены и служат входом теста миграции 2 → 3 (`tests/migration_v2.rs`).

Как получены (временный тест `tests/gen_v2_tmp.rs`, удалён сразу после генерации; запуск
`GEN_OUT=<папка fixtures> cargo test -p look-presets --test gen_v2_tmp` ДО правок схемы):

1. `PresetFileStore::open(<временная папка>, &PresetStoreConfig::default(), PRODUCT_MIGRATIONS)`.
2. `with_edits`: `SetLook` (контраст +0,3, теплота −0,25, полоса `sky` оттенок +0,4, Lift толчок (0,3; −0,2),
   Gain кольцо +0,15) → `SaveAsNewPreset("Evening")` → `SetLook` (контраст −0,5, Offset толчок (−0,1; 0,1)) → `save`.
3. `no_edits`: `SetLook` (контраст +0,6) → `SaveAsNewPreset("Punchy")` → `save` (`edits: null`).

Файлы не править.
