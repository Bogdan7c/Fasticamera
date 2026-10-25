# Fasticamera — core (точка входа)

Короткая карта текущего состояния (граф ссылок, не журнал).

## Проект
- Личная камера для OnePlus 11 (CPH2447, salami), Rust + Vulkan (ash), в магазин не публикуется. Приватный репозиторий GitHub `Bogdan7c/Fasticamera`, ветка `main`. Имя пакета приложения — `dev.fasticamera` (решение 20).
- Цель: сырой поток с сенсора без копий в Vulkan, вся обработка своя, снаружи — клон камеры iOS 26 (Liquid Glass).
- **Конечная цель владельца — паритет с iPhone по видео** (решение 16); затем фото «как iPhone» (решение 17). Костяк обязан выполнять задел V1–V10 (README плана).
- Действующий план — `user/foundation/` (README: решения владельца 1–24, архитектура, владельцы состояния, инварианты, задел V1–V10, порядок сессий 00–17, общие правила, бэклог).
- **Сессия 00 завершена (2026-10-25): GO** — 60 к/с RAW без root, импорт в Vulkan без копий, Vulkan → HEVC-кодер. Отчёт: `user/foundation/results/00.md`. Дневной набор C7 доснимается 26.10 (решение 24).
- Кода продукта пока нет. Есть только зонд разведки `tools/device-probe` (Kotlin + Rust `probe-native` + `pc/probe.py`; сборка: `ANDROID_HOME=$HOME/Android/Sdk JAVA_HOME=/usr/lib/jvm/java-21-openjdk ./gradlew assembleDebug test`, `cd native && cargo test`).
- Факты о телефоне: `mem:device/oneplus11-camera-facts`.

## Ключевые решения (детали — README плана)
- Без root. Хак имени пакета не нужен (решение 20).
- Управление камерой — Camera2 NDK; первая задача 04 — подтвердить 60 к/с через NDK, иначе стоп-вопрос (решение 22).
- RAW-поток: RAW10 по умолчанию, RAW16 через конфиг (решение 21).
- Фото-видоискатель основной: 4:3@60 через ключи CamX за `VendorCapabilities` с откатом на 4:3@30; ширик 4:3@60 и видео 16:9@60 — штатно (решение 23).
- UI гибрид: экран камеры — свой Rust/Vulkan UI-движок со стеклом; настройки — Compose.
- Видоискатель только из RAW через наш ISP; AF/AE вендорские, AWB и цвет наши. Вендорская коррекция виньетирования в RAW неотключаема — учитывается в паспорте кадра.
- Тексты UI английские; документация и комментарии русские.

## Тулчейн (установлен в 00)
- SDK `~/Android/Sdk` (platform 36, build-tools 36.1.0, NDK 30.0.16248370), JDK 21, Gradle 9.8.2 wrapper, AGP 9.4.1 (встроенный Kotlin), Rust таргет `aarch64-linux-android`, cargo-ndk 4.1.2, jadx `~/.local/share/jadx`.

## Процесс
- Правила `AGENTS.md`; Context7 перед кодом; широкий поиск — read-only субагентом Sonnet.
- Коммиты только по команде владельца, без веток; push — по поручению. В `user/` только `.md`, без дампов/логов/личных путей; сырые данные — `~/fasticamera-captures/`.
- Отчёты сессий — `user/foundation/results/NN.md`; межплановый бэклог — `user/general_problems/general_backlog.md`.
- Serena onboarding и memories `conventions`, `task_completion`, `suggested_commands`, `tech_stack` — создаются в сессии 01.
