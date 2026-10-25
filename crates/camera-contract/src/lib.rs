//! `camera-contract` — нейтральный словарь камеры Fasticamera.
//!
//! Здесь будут жить типы, которыми обмениваются `camera-core`, `camera-android`, ISP и UI:
//! ID и роли камер, возможности, паспорт RAW-кадра (размер, мозаика, black/white level,
//! время, экспозиция, флаги вендорской обработки), намерения управления.
//! Наполняется в сессии 04 (`user/foundation/04-camera-backend-ndk.md`).
//!
//! Инвариант: crate не зависит ни от Android, ни от Vulkan, ни от других crate-ов workspace —
//! это проверяет `scripts/check-crate-boundaries.py` (правила в `scripts/crate-boundaries.toml`).
