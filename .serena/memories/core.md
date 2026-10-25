# Fasticamera — core (точка входа)

Короткая карта текущего состояния (граф ссылок, не журнал).

## Проект
- Личная камера для OnePlus 11 (CPH2447, salami), Rust + Vulkan (ash), в магазин не публикуется. Приватный репозиторий GitHub `Bogdan7c/Fasticamera`, ветка `main`. Пакет `dev.fasticamera` (debug: `dev.fasticamera.debug`).
- Цель: сырой поток с сенсора без копий в Vulkan, вся обработка своя, снаружи — клон камеры iOS 26 (Liquid Glass). Конечная цель владельца — паритет с iPhone по видео (решение 16), затем фото «как iPhone» (17); костяк обязан выполнять задел V1–V10.
- Действующий план — `user/foundation/` (README: решения 1–25, архитектура, владельцы состояния, инварианты, V1–V10, порядок сессий 00–17, общие правила, бэклог). Отчёты — `user/foundation/results/NN.md`.
- Сделано: 00 (GO: 60 к/с RAW без root, импорт в Vulkan без копий, Vulkan → HEVC), 01 (скелет: workspace, Gradle + cargo-ndk, линты, CI-скрипт, guardrail, ratchet, деплой, smoke, логи в logcat). Следующая по маршруту — 02 (android-shell, JNI-шов, surface).
- Факты о телефоне: `mem:device/oneplus11-camera-facts`.

## Карта кода (сейчас)
- `crates/camera-contract` — пустой каркас словаря (наполняется в 04).
- `crates/fasticamera-app` — composition root, `libfasticamera.so`: `jni_exports` (android) → `jni_boundary::run_guarded`; `runtime` (конфиг на процесс), `runtime_config`, `logging/` (tracing → свой слой → logcat/stderr), `panic_report` (хук → FATAL в logcat + отчёт), `scenario/` (smoke `hello`, `panic` за фичей), `result_line` (`FASTICAMERA_RESULT {json}`).
- `android/` — тонкая `MainActivity` + `NativeBridge` (`initialize(configJson)`, `runScenario(name)`) + `NativeConfig`.
- `scripts/` — CI, guardrail, ratchet, deploy/smoke, тесты в `scripts/tests/`.
- `tools/device-probe` — зонд сессии 00, отдельный проект.

## Где что
- Версии, профили сборки, варианты APK: `mem:tech_stack`.
- Команды (CI, тесты, деплой, smoke, коды выхода): `mem:suggested_commands`.
- Правила кода, границы JNI, логирование, конфиг, стиль скриптов: `mem:conventions`.
- Чеклист завершения задачи: `mem:task_completion`.

## Ключевые решения (детали — README плана)
- Без root; хак имени пакета не нужен (20). Камера — Camera2 NDK (22). RAW10 по умолчанию, RAW16 через конфиг (21). Фото-видоискатель основной 4:3@60 через ключи CamX за `VendorCapabilities` (23).
- UI гибрид: экран камеры — свой Rust/Vulkan UI со стеклом; настройки — Compose. Видоискатель только из RAW через наш ISP; AF/AE вендорские, AWB и цвет наши. Вендорская коррекция виньетирования в RAW неотключаема — в паспорт кадра.
- Эталон «как iPhone» — парные съёмки с iPhone 17 Pro Max с двойного крепления; медиа только вне репо (решение 25).
- Сессия 01: паника ловится на границе JNI (не abort); debug и release стоят рядом; мост логов в logcat — свой.

## Ловушки окружения
- Буфер logcat `main` на телефоне 256 КиБ: при активном использовании телефона записи вытесняются за секунды. Читать logcat потоком с `-T <epoch>`, не `-d` постфактум; `logcat -c`/`-G` не трогать (изменение системы).
- Телефон — личный и в использовании: запускать только свои APK, скриншоты не снимать без нужды (видны чужие уведомления).

## Процесс
- Правила `AGENTS.md`; Context7 перед кодом; широкий поиск — read-only субагентом Sonnet. Serena: языковых серверов в `.serena/project.yml` нет — символьные инструменты по коду пока не работают.
- Коммиты только по команде владельца, без веток; push — по поручению. В `user/` только `.md`; сырые данные — `~/fasticamera-captures/`. Межплановый бэклог — `user/general_problems/general_backlog.md`.
