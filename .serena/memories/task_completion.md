# Завершение задачи

1. `cargo fmt --all`.
2. `scripts/ci-checks.sh all` = 0 (fmt, clippy host+android × без фич/все фичи, тесты × без фич/все фичи, тесты скриптов, guardrail, ratchet, оба APK).
3. Если менялся Rust/Android-путь и телефон подключён: `scripts/device-smoke.sh hello` и `scripts/device-smoke.sh --no-build panic` (debug), при необходимости `--variant release hello`.
4. Фикс/оптимизация — тест, который падает без фикса; проверить временным откатом.
5. Новый crate → запись в `scripts/crate-boundaries.toml`; новый внешний крейт → зрелость/авторы/лицензия в отчёте сессии.
6. Self-review диффа; обновить Serena memories, если поменялись границы, API, команды, инварианты.
7. Отчёт сессии `user/foundation/results/NN.md` (только .md, без личных путей/serial/логов). Сессия закрыта (всё зелёное, отчёт и memories готовы) → сразу коммит в `main` и `git push origin main` без отдельного одобрения (решение 44 README); незакрытую не коммитить.
