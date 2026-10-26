# Команды

Оболочка владельца — fish (`$PIPESTATUS`, heredoc-ы bash в ней не работают; скрипты запускать как файлы).

- Все проверки без телефона: `scripts/ci-checks.sh all` (стадии: fmt, clippy-host, clippy-android, test, scripts-test, boundaries, module-size, apk; можно перечислить отдельные стадии).
- Rust на ПК: `cargo test --workspace` и `cargo test --workspace --all-features` (фича `debug-scenarios` включает отладочные сценарии).
- Тесты скриптов: `python3 -m unittest discover -s scripts/tests -t scripts/tests`.
- Guardrail границ: `python3 scripts/check-crate-boundaries.py [--offline] [--manifest-path X]`; правила — `scripts/crate-boundaries.toml`.
- Ratchet размера: `python3 scripts/module-size-check.py [--update-baseline | --adopt PATH]`; политика и baseline — `scripts/module-size-policy.toml` (`[baseline]` всегда последняя секция).
- APK: `cd android && ./gradlew assembleDebug assembleRelease` (нужны `ANDROID_HOME`, `JAVA_HOME`; скрипты выставляют дефолты сами через `scripts/lib/common.sh`).
- Телефон:
  - `scripts/device-deploy.sh [--variant debug|release] [--no-build] [--log-seconds N]` — сборка, установка, запуск, logcat тега `Fasticamera`.
  - `scripts/device-smoke.sh [--variant ...] [--no-build] [--timeout S] <сценарий>` (`--no-build` ставит последний собранный APK). Сценарии: `hello`, `panic` (только debug), `lifecycle` (скрипт сам жмёт HOME и возвращает), `permission` (`--timeout 60`; сбрасывает и возвращает разрешение камеры только нашего пакета, сам жмёт «Don't allow»; экран разблокирован, телефон не трогать; нужна опция разработчика OxygenOS), `motion` (гироскоп ≥ 380 Гц; ориентация `unknown`, если телефон лежит), `platform` (тепло, 120 Гц, проигрывает 9 вибраций) — коды выхода: 0 прошёл, 1 упал, 2 нет устройства, 3 нет строки результата, 4 сборка/установка/запуск, 64 неверный вызов.
  - Сценарии сессии 03: `render` (`--timeout 120`; ритм 10 с, период ≈ 16,67 мс при цели 60 Гц), `render-lifecycle` (`--timeout 400`; 20× HOME + 3× экран кнопкой питания; в конце телефон на блокировке), `ahb-import`, `gpu-fence` (`--timeout 60`). Ритм сравнивать на release (`--variant release`), validation — на debug.
  - Validation layer для debug APK: `scripts/fetch-validation-layer.sh` (кэш `~/.cache/fasticamera/vulkan-validation-layers/<ver>/`, офлайн при кэше; вызывается из `apk`-стадии CI и `build_apk debug`).
  - Сырые времена показа: запуск с `-e log_filter "info,fasticamera::gpu::display_timing=trace"`; шаги создания GPU-контекста — `fasticamera::gpu::init=debug`.
  - Переопределить фильтр логов на один запуск: `adb shell am start -S -n dev.fasticamera.debug/dev.fasticamera.MainActivity -e log_filter "info,fasticamera::scenario=trace"`.
- Устройство выбирается по модели `CPH2447`; несколько — задать `ANDROID_SERIAL`.
