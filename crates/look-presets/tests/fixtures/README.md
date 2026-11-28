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

## Схема 3

`schema_v3_with_edits.json` и `schema_v3_no_edits.json` — настоящие файлы схемы 3, записанные кодом `look-presets` на
коммите `0552fb2` (до новых краёв ручек, сессия 14l). Схема в 14l НЕ поднята: состав файла прежний, изменился смысл
чисел (что значит край ±1 — модель вида, а не файл), а значения владелец велел не пересчитывать (решение 237). Файлы
доказывают, что старый сохранённый вид читается без сбоя запуска и без правки чисел (`tests/schema_v3.rs`).

Как получены (временный тест `tests/gen_v3_tmp.rs` в отдельном worktree коммита, удалён):

1. `PresetFileStore::open(<временная папка>, &PresetStoreConfig::default(), &environment(), PRODUCT_MIGRATIONS)`.
2. `with_edits`: `SetLook` (контраст +1, тени +1, света −0,6, теплота −0,25, полоса `sky` оттенок +0,4, Lift толчок
   (0,3; −0,2), Gain кольцо +0,15) → `SetToneBase(Aces)` → `SaveAsNewPreset("Evening")` → `SetKnob` контраст −0,5 →
   `save`.
3. `no_edits`: `SetLook` (контраст +0,6) → `SaveAsNewPreset("Punchy")` → `save` (`edits: null`).

Файлы не править.

## Схема 4

`schema_v4_with_edits.json` и `schema_v4_no_edits.json` — настоящие файлы схемы 4 (полоски Lift / Gamma / Gain / Offset
по каналам R, G, B, сессия 14t), записанные кодом `look-presets` на коммите `7bffc52` + правки 14t (схема 4), а не
набранные руками. Ненулевые полоски нужны, чтобы тест «пережил перезапуск» ловил потерю или перестановку любой из них
(`tests/schema_v4.rs`). Когда схема поднимется до 5, эти файлы станут входом миграции 4 → 5 и замораживаются.

Как получены (временный тест `tests/gen_v4_tmp.rs`, удалён сразу после генерации; запуск
`GEN_OUT=<папка fixtures> cargo test -p look-presets --test gen_v4_tmp`; `PresetFileStore::open` в пустой папке
с `PresetStoreConfig::default()` и окружением тестов — основа AgX, полосы `sky`, `green`):

1. `with_edits`: `SetKnob` контраст +1, тени +1, света −0,6, теплота −0,25; `SetPrimary` Lift/R +0,3, Gain/B −0,45;
   `SetToneBase(Aces)` → `SaveAsNewPreset("Evening")` → правки поверх: контраст −0,5, `SetPrimary` Offset/G +0,2,
   Lift/R −0,1 → `save`.
2. `no_edits`: `SetKnob` контраст +0,6, `SetPrimary` Gamma/R +0,5 → `SaveAsNewPreset("Punchy")` → `save` (`edits: null`).

Файлы не править.
