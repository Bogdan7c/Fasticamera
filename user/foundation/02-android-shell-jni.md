# Сессия 02: Android-оболочка и JNI-шов

Приложи этот файл и скопируй абзац ниже в сообщение.

> Выполни сессию 02 плана `user/foundation/`: сделай тонкую Kotlin-Activity и crate `android-shell` — типизированный шов между Android и Rust (жизненный цикл, surface, касания, разрешение камеры, хаптика, ориентация, тепло, частота экрана). Бизнес-логики в Kotlin нет. Сначала прочитай `user/foundation/README.md`, `results/00.md` и `results/01.md`.

## Зачем

Шов Android ↔ Rust — главная граница приложения. Если на ней смешаются вызовы JNI, решения о жизненном цикле и логика фич, каждый следующий модуль начнёт «немного знать про Android». Цель — чтобы весь Rust выше шва видел только нейтральные события и команды.

## Наследство

- 00: какие хаптические примитивы поддерживаются, как включается 120 Гц для `SurfaceView`, API теплового запаса.
- 01: сборка, логирование, `device-smoke.sh`.

## Архитектура

- **Kotlin** (`android/`):
  - Activity в полноэкранном режиме edge-to-edge, с учётом выреза под камеру;
  - один `SurfaceView` на весь экран;
  - системный запрос разрешения;
  - передача касаний (`MotionEvent` → компактная запись: id пальца, x, y, время, фаза) без обработки.
  - Портретная ориентация зафиксирована. iOS-камера не поворачивает раскладку, а поворачивает иконки, поэтому нужна **физическая ориентация** с датчика.
- **`android-shell`** (Rust) — единственный, кто знает про `jni` и `ndk`. Наружу отдаёт:
  - `ShellEvent`: `SurfaceCreated(NativeSurface)`, `SurfaceChanged`, `SurfaceDestroyed`, `Resumed`, `Paused`, `Touch(TouchSample)`, `DeviceOrientation(…)`, `ThermalStatus(…)`, `CameraPermission(Granted | Denied | DeniedPermanently)`.
  - `ShellCommands` (intent-методы): `request_camera_permission()`, `play_haptic(HapticPattern)`, `set_preferred_frame_rate(FrameRate)`, `open_system_gallery()` (решение: в v1 миниатюра открывает системную галерею), `keep_screen_on(…)`.
  - Сервис-трейты (`HapticsService`, `ThermalSource`) — чтобы потребители тестировались с fake.
- **Время жизни surface**: `SurfaceDestroyed` синхронный, Android ждёт возврата. Рендер должен успеть отпустить swapchain. Это документированный контракт шва (подтверждение «отпустил» — от владельца рендера, сессия 03).
- Касания: время — монотонное (`MotionEvent.getEventTimeNanos`), координаты в пикселях surface. Перевод в точки (pt) — в `ui-core`, не здесь.

## Что сделать

1. Kotlin-Activity и JNI-регистрация (`RegisterNatives`, без поиска по имени символа в рантайме).
2. `android-shell`: события, команды, сервисы, safety-модуль для JNI (`// SAFETY:`, глобальные ссылки освобождаются).
3. Поток событий в Rust — ограниченный канал. Переполнение касаниями считается и логируется, а не блокирует главный поток.
4. Хаптика: `HapticPattern::{SelectionTick, LightImpact, …}` → примитивы, поддержанные по 00. Явный fallback.
5. Тепло: подписка на `OnThermalStatusChangedListener` и периодический `getThermalHeadroom`. Тепловая политика (V8 README) — отдельный владелец (`ThermalPolicy`), фичи спрашивают у него, а не читают датчики сами.
6. **Гироскоп (V3)**: `MotionSource` на `ASensorManager` (NDK, без JNI на горячем пути), 400 Гц, метки времени в шкале `CLOCK_BOOTTIME` (та же, что у `SENSOR_TIMESTAMP`, V2). Кольцо последних секунд в нейтральном типе. Тест: синтетический поток → выборка интервала по времени; на телефоне — фактическая частота ≥ 380 Гц.

## Тесты

- Протокол касаний: сериализация и десериализация последовательности (down/move/up/cancel, мультитач) → те же записи; поломка порядка → тест падает.
- Fake `HapticsService`: команда фичи → ожидаемый паттерн. Отсутствующий примитив → fallback, а не молчание.
- Переполнение канала: N+1 событие → счётчик потерь = 1, главный поток не блокировался.
- На телефоне `device-smoke.sh lifecycle`: свернуть/развернуть (через `input keyevent`) → в логе строго `Paused → SurfaceDestroyed → … → SurfaceCreated → Resumed` в правильном порядке, без утечек глобальных JNI-ссылок (счётчик).
- `device-smoke.sh permission`: отказ → `Denied`, повторный → `DeniedPermanently`. Сценарий сбрасывает разрешение только **нашего** пакета (`pm revoke`).

## Критерий выхода

- Никакой crate, кроме `android-shell` и `fasticamera-app`, не зависит от `jni`/`ndk` (guardrail).
- Smoke-сценарии жизненного цикла и разрешения зелёные на телефоне.

Memories: `mem:core`, `mem:conventions`.
