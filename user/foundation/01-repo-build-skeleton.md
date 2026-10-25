# Сессия 01: скелет репозитория, сборки и проверок

Приложи этот файл и скопируй абзац ниже в сообщение.

> Выполни сессию 01 плана `user/foundation/`: создай Cargo workspace, Gradle-проект `android/` со сборкой Rust через cargo-ndk, политику линтов, `scripts/ci-checks.sh`, guardrail границ crate-ов, ratchet размера модулей, деплой и логирование в logcat. В итоге пустое приложение ставится на телефон одной командой, грузит Rust-библиотеку и пишет строку в logcat. Сначала прочитай `user/foundation/README.md` и `results/00.md`.

## Зачем

Дисциплина закладывается до первой фичи. Если линты, guardrail-ы и деплой появятся позже, первые пять сессий успеют набрать долг, который потом придётся разгребать отдельно. Цель — чтобы правило README «нейтральные ядра не зависят от Android» проверялось скриптом, а не памятью агента.

## Наследство сессии 00 (`results/00.md`)

- Версии тулчейна (Rust, NDK, AGP, Gradle, JDK, cargo-ndk) — брать оттуда, не выбирать заново.
- Имя пакета (`applicationId`) — из решения владельца по итогам 00.
- Способ управления камерой (NDK, Java или гибрид) влияет на то, сколько Kotlin будет в `android/`. Здесь закладывается только каркас.

## Что сделать

1. **Workspace**:
   - `Cargo.toml` с `[workspace.lints]`: clippy `unwrap_used`/`expect_used`/`panic` = warn, `unsafe_op_in_unsafe_fn` = deny. В `ci-checks` — `-D warnings`.
   - Edition 2024 и закреплённый `rust-toolchain.toml`.
   - Пустые crate-ы создавать **не нужно**. Здесь появляются только `fasticamera-app` (cdylib, точка входа JNI) и `camera-contract` — каркас словаря, наполняется в 04.
2. **Gradle** (`android/`):
   - wrapper, AGP, Kotlin;
   - `minSdk = targetSdk = 36` — один телефон, поддержка старых версий не нужна;
   - задача сборки Rust через `cargo-ndk` перед `merge*JniLibFolders`, ABI только `arm64-v8a`;
   - debug и release с понятными отличиями (release — LTO, без отладочных символов в APK, символы рядом для `ndk-stack`).
3. **Логирование**: `tracing` → logcat (тег `Fasticamera`, уровни из config, а не хардкодом). Panic hook пишет в logcat место и сообщение до аварийного завершения.
4. **Скрипты**:
   - `scripts/ci-checks.sh all`: fmt, clippy (host и `aarch64-linux-android`), тесты на ПК, сборка APK, guardrail, ratchet.
   - `scripts/check-crate-boundaries.py`: правила зависимостей из README. Пример: `camera-core`, `isp-model`, `ui-core`, `camera-screen` не зависят от `ndk`, `jni`, `ash`, `camera-android`, `android-shell`. Правила — в файле данных, а не в коде скрипта.
   - `scripts/module-size-check.py`: мягкий 700 / жёсткий 800, baseline-файл.
   - `scripts/device-deploy.sh`: сборка, установка, запуск, фильтр logcat.
   - `scripts/device-smoke.sh <сценарий>`: запуск сценария по intent-параметру, ожидание структурированной строки `FASTICAMERA_RESULT {json}`, код выхода по результату.
5. **AGENTS.md**: в разделе о размере модулей упомянуты crate-ы Fastiplayer. Предложить владельцу правку на crate-ы Fasticamera (`camera-core`, `camera-android`, `isp-vulkan`, `ui-render-vulkan`, `camera-screen`, `fasticamera-app`). Править только после «да».
6. **Serena**: onboarding проекта, первые memories — `core` (граф ссылок), `conventions`, `task_completion`, `suggested_commands`, `tech_stack`.
7. **`.gitignore`**: `target/`, `android/build`, `*.apk`, локальные дампы. Проверить, что игнор не глотает исходники (урок fastiplayer: `target/` глотал `external_open/target/tests.rs`).

## Тесты

- Guardrail: фикстура с запрещённой зависимостью → скрипт падает с понятным сообщением; разрешённая → проходит. Это тест самого скрипта.
- Ratchet: файл 801 строка → ошибка; рост legacy-файла → ошибка.
- `device-smoke.sh hello` на телефоне: Rust-библиотека загружена, строка результата содержит версию сборки и `abi=arm64-v8a`. Без телефона сценарий явно сообщает «устройство не найдено» (код выхода отличный от «упал тест»).
- Panic hook: тестовый сценарий с намеренной паникой в отладочной сборке → в logcat место и сообщение.

## Критерий выхода

- Из чистого checkout: `scripts/ci-checks.sh all` = 0, `scripts/device-deploy.sh` ставит и запускает приложение.
- Guardrail и ratchet работают и покрыты тестами.
- Memories созданы, `mem:core` указывает на план.

Memories: `mem:core`, `mem:conventions`, `mem:task_completion`.
