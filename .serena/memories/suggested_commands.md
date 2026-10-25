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
  - `scripts/device-smoke.sh [--variant ...] [--no-build] [--timeout S] <hello|panic>` — коды выхода: 0 прошёл, 1 упал, 2 нет устройства, 3 нет строки результата, 4 сборка/установка/запуск, 64 неверный вызов.
  - Переопределить фильтр логов на один запуск: `adb shell am start -S -n dev.fasticamera.debug/dev.fasticamera.MainActivity -e log_filter "info,fasticamera::scenario=trace"`.
- Устройство выбирается по модели `CPH2447`; несколько — задать `ANDROID_SERIAL`.
