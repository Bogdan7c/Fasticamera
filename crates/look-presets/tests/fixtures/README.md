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
