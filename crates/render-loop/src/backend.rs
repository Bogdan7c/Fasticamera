//! Контракт бэкенда показа: всё, что поток рендера знает о GPU.
//!
//! Поток зависит только от этого трейта (а не от `gpu-vulkan`): настоящий бэкенд живёт
//! в `vulkan_backend` и сам переводит свои типы в нейтральные структуры отсюда; в тестах
//! подставляется fake. Исходы разведены по смыслу (успех / пересоздать / surface потеряна /
//! таймаут / ошибка) и не сливаются в `bool`: поток реагирует на каждый по-своему.

use std::collections::BTreeMap;
use std::time::Duration;

use shell_contract::clock::MonotonicNanos;

use crate::camera_diagnostics::CameraStreamDiagnostics;
use crate::config::LateLatchMode;
use crate::still::{StillCommand, StillCommandOutcome, StillWorkload};
use crate::still_diagnostics::StillDiagnostics;
use crate::thread::{SleepUntil, SleepUntilOutcome, SystemMonotonicTimer, sleep_until};
use crate::ui_overlay::{UiOverlayCommand, UiOverlayOutcome};
use crate::viewfinder::{ViewfinderCommand, ViewfinderCommandOutcome, ViewfinderDiagnostics};
use shell_contract::surface::{NativeSurface, SurfaceSize};

/// Что известно потоку о кадре, который нужно нарисовать.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameTiming {
    /// Порядковый номер кадра, возрастает на каждый вызов `render_and_present` (с нуля).
    pub frame_index: u64,
    /// Номер тика vsync, по которому рисуется кадр.
    pub vsync_sequence: u64,
    /// Сколько тиков vsync пропущено между предыдущим кадром и этим.
    pub skipped_vsyncs: u64,
    /// Выбранная линия кадра и план позднего захвата (сессия 07).
    pub target: PresentTarget,
}

impl FrameTiming {
    /// Когда ожидается показ кадра (основа `desiredPresentTime` для `VK_GOOGLE_display_timing`):
    /// момент показа ВЫБРАННОЙ линии кадра.
    pub fn expected_present(&self) -> MonotonicNanos {
        self.target.expected_present
    }
}

/// Как выбрана линия кадра (см. `config::PresentTimelinePolicy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetChoice {
    /// Политика `Preferred`: линия, которую предложила платформа.
    PlatformPreferred,
    /// Политика `NearestFeasible`: самая ранняя линия, к сроку которой успеваем.
    NearestFeasible,
    /// Политика `NearestFeasible`, но ни одна линия не выполнима: взята предпочитаемая.
    FallbackToPreferred,
}

/// Линия кадра, на которую метит этот кадр, и план позднего захвата кадра камеры.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentTarget {
    /// Индекс линии в `timelines.as_slice()` тика vsync.
    pub timeline_index: usize,
    /// Индекс предпочитаемой платформой линии в том же списке (для сравнения в диагностике).
    pub preferred_index: usize,
    pub choice: TargetChoice,
    /// Идентификатор линии в SurfaceFlinger.
    pub vsync_id: i64,
    /// Когда кадр этой линии появится на экране.
    pub expected_present: MonotonicNanos,
    /// К какому моменту кадр должен быть готов, чтобы попасть в `expected_present`.
    pub deadline: MonotonicNanos,
    /// Когда брать кадр камеры (самый поздний безопасный момент).
    pub latch: FrameLatchPlan,
}

impl PresentTarget {
    /// План, при котором кадр камеры берётся сразу (без позднего захвата): срок захвата —
    /// начало эпохи часов, то есть «уже прошёл». Для источников и тестов без линий кадра.
    pub fn latch_immediately(expected_present: MonotonicNanos, deadline: MonotonicNanos) -> Self {
        Self {
            timeline_index: 0,
            preferred_index: 0,
            choice: TargetChoice::PlatformPreferred,
            vsync_id: 0,
            expected_present,
            deadline,
            latch: FrameLatchPlan {
                latch_at: MonotonicNanos::from_nanos(0),
                spin_window: Duration::ZERO,
                max_wait: Duration::ZERO,
                mode: LateLatchMode::Disabled,
            },
        }
    }
}

/// План позднего захвата: самый поздний момент взять кадр камеры, чтобы успеть к сроку
/// выбранной линии (`latch_at = deadline − оценка стоимости кадра − запас`).
///
/// При `mode == BeforeCameraPoll` бэкенд ждёт до `latch_at` через
/// [`FrameLatchPlan::wait_until_latch`] и только потом опрашивает камеру (Vulkan-бэкенд делает
/// это сам перед вызовом сессии камеры). Ждать на GPU нельзя: это ожидание только CPU-потока
/// рендера. Повторный вызов после `latch_at` возвращается сразу — двойное ожидание безвредно.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameLatchPlan {
    /// Самый поздний момент опроса камеры (`CLOCK_MONOTONIC`).
    pub latch_at: MonotonicNanos,
    /// Последние столько ожидания — активно, без сна (`present_target.latch_spin_us`).
    pub spin_window: Duration,
    /// Дольше этого не ждать никогда (`present_target.latch_max_wait_us`).
    pub max_wait: Duration,
    /// Ждать ли `latch_at` (`present_target.late_latch`).
    pub mode: LateLatchMode,
}

impl FrameLatchPlan {
    /// Ждёт до `latch_at` по системным монотонным часам (сон, затем активное ожидание).
    pub fn wait_until_latch(&self) -> SleepUntilOutcome {
        sleep_until(
            SleepUntil {
                target: self.latch_at,
                spin_window: self.spin_window,
                max_wait: self.max_wait,
            },
            &SystemMonotonicTimer,
        )
    }
}

/// Замер CPU одного кадра бэкендом: на нём поток строит оценку стоимости кадра и диагностику.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameCpuTiming {
    /// Время внутри `acquire_frame` (ожидание fence слота кольца + `vkAcquireNextImageKHR`).
    pub acquire_wait: Duration,
    /// Сколько поток ждал момента позднего захвата после acquire (ноль — не ждал).
    pub latch_wait: Duration,
    /// Чем кончилось ожидание позднего захвата (отказы ждать считаются в диагностике).
    pub latch_result: LatchWaitResult,
    /// CPU от захвата кадра камеры (конец ожидания `latch_at`, а без ожидания — конец acquire) до
    /// возврата из сабмита и показа. Ожидание в стоимость не входит: иначе оценка росла бы от
    /// собственного ожидания. `None` — кадр до показа не дошёл (acquire не удался).
    pub latch_to_submit: Option<Duration>,
}

/// Время кадра на GPU (метки начала и конца команд кадра), выдаётся бэкендом ПОЗЖЕ самого кадра:
/// метки слота читаются только после его fence (без ожиданий GPU), то есть через 1–3 кадра.
/// Поток рендера кладёт его в оценку стоимости кадра и в отчёт ADPF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameGpuTiming {
    /// Номер кадра потока (`FrameTiming::frame_index`), чьи команды измерены.
    pub frame_index: u64,
    /// Длительность команд кадра на GPU (от первой до последней метки кадра).
    pub gpu_duration: Duration,
}

/// Чем кончилось ожидание позднего захвата в кадре. Варианты различимы: «не просили»,
/// «дождались», «отказались ждать» и «часы сломаны» считаются по-разному.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LatchWaitResult {
    /// Ожидание не требовалось: поздний захват выключен, камера не рисуется или кадр не дошёл до
    /// захвата (acquire не удался).
    #[default]
    NotRequested,
    /// Дождались момента захвата (или он уже прошёл к началу ожидания).
    Waited,
    /// Срок захвата дальше предела ожидания (`FrameLatchPlan::max_wait`): камера опрошена сразу.
    RefusedBeyondBudget,
    /// Часы не прочитались: камера опрошена сразу.
    ClockFailed,
}

/// Результат попытки нарисовать и показать кадр.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresentOutcome {
    /// Кадр поставлен на показ.
    Presented,
    /// Тик пропущен: на экране ничего бы не изменилось (нет нового кадра камеры, список рисования UI и всё остальное,
    /// что влияет на пиксели, совпадают с последним показанным кадром), поэтому кадр не записан и не показан, а экран
    /// продолжает держать прежний (решение 214). Это не потеря кадра и не ошибка: счётчик показов не растёт,
    /// пейсинг учитывает пропущенные тики как задуманные, а оценка стоимости кадра их не видит.
    SkippedUnchanged,
    /// Swapchain устарел (`SUBOPTIMAL`/`OUT_OF_DATE`/смена размера): нужно пересоздать
    /// цепочку для текущего размера. Кадр не показан.
    NeedsRecreate,
    /// Surface потеряна (`ERROR_SURFACE_LOST_KHR`): бэкенд её больше использовать не может.
    SurfaceLost,
    /// Картинка swapchain не освободилась за `acquire_timeout`: кадр пропущен, это не ошибка.
    TimedOut,
    /// Другая ошибка кадра (не потеря surface). Кадр не показан; поток считает ошибку.
    Failed(BackendError),
}

/// Ошибка бэкенда с человекочитаемой причиной (для статуса и логов).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{operation}: {reason}")]
pub struct BackendError {
    /// Что делали («create_swapchain», «queue_present»…).
    pub operation: String,
    /// Что пошло не так.
    pub reason: String,
}

/// Отказ `attach`. К моменту возврата бэкенд ничего не держит: переданная surface уже
/// уничтожена (отпущена), swapchain не создан.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AttachError {
    /// Surface непригодна для показа (например, окно уже умерло).
    #[error("surface не принята бэкендом: {0}")]
    SurfaceRejected(BackendError),
    /// GPU не смог создать цепочку показа.
    #[error("не удалось создать цепочку показа: {0}")]
    SwapchainFailed(BackendError),
}

/// Момент реального показа кадра (по `VK_GOOGLE_display_timing`, `CLOCK_MONOTONIC`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PastPresentation {
    /// Номер показа в цепочке бэкенда: растёт на 1 за каждый показанный кадр (циклический
    /// `u32`), начинается заново после пересоздания/присоединения цепочки. Соседние номера —
    /// соседние кадры; по нему пейсинг отличает честный интервал от дыры в записях драйвера.
    pub present_id: u32,
    /// Номер кадра потока, который запросил показ.
    pub frame_index: u64,
    /// Когда кадр реально появился на экране.
    pub actual_present: MonotonicNanos,
}

/// Что бэкенд знает о текущей цепочке показа. Нейтральные типы: форматы и повороты —
/// именами (для логов и отчётов смоков), чтобы поток и сценарии не зависели от `gpu-vulkan`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayInfo {
    /// Имя формата картинок swapchain (например, `B8G8R8A8_UNORM`).
    pub format: String,
    /// Имя цветового пространства (например, `SRGB_NONLINEAR_KHR`).
    pub color_space: String,
    /// Физический размер картинок (с учётом поворота дисплея).
    pub extent: SurfaceSize,
    /// Имя поворота дисплея, который учитывает рисование (`Identity`, `Rotate90`…).
    pub transform: String,
    /// Сколько картинок в цепочке.
    pub image_count: usize,
    /// Сколько раз цепочка пересоздавалась с момента `attach`.
    pub recreation_count: u64,
    /// Реальный период обновления экрана, если платформа его сообщает
    /// (`VK_GOOGLE_display_timing`); `None` — неизвестен (ПК-стенд). Пейсинг судит по нему,
    /// а `preferred_frame_rate_hz` берёт только как запасной вариант.
    pub refresh_cycle: Option<Duration>,
}

/// Снимок диагностики бэкенда: текущие значения счётчиков и параметров цепочки.
/// Времена показов сюда не входят: их вычерпывает [`DisplayBackend::drain_past_presentations`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BackendDiagnostics {
    /// Сколько Vulkan-объектов (или их аналогов) сейчас живо у бэкенда; после `detach`
    /// должно вернуться к исходному значению.
    pub live_objects: u64,
    /// Те же живые объекты в разрезе вида (`Fence`, `Image`, …); виды с нулём не перечисляются.
    /// Нужны, чтобы сравнивать состояние «в покое» целиком, а не только сумму: утечка одного
    /// вида не должна прятаться за освобождением другого.
    pub live_objects_by_kind: BTreeMap<String, u64>,
    /// Накопленное число ошибок validation.
    pub validation_errors: u64,
    /// Накопленное число предупреждений validation.
    pub validation_warnings: u64,
    /// Сколько раз `detach` не смог корректно уничтожить цепочку (GPU не успел за таймаут
    /// и т. п.). Окно при этом всё равно возвращено, но объекты GPU могли остаться.
    pub detach_failures: u64,
    /// Параметры текущей цепочки показа; `None` — surface не присоединена.
    pub display: Option<DisplayInfo>,
    /// Сколько раз сработал каждый ЗАДОКУМЕНТИРОВАННЫЙ известный отход validation (ключ — VUID,
    /// 0 — не срабатывал). Справочные числа: ошибкой не считаются.
    pub known_deviations: BTreeMap<String, u64>,
    /// Показ кадров камеры; `None` — бэкенд показывает тестовый паттерн.
    pub camera: Option<CameraStreamDiagnostics>,
    /// Видоискатель (ISP, компоновка, параметры); `None` — бэкенд не показывает камеру.
    pub viewfinder: Option<ViewfinderDiagnostics>,
    /// Снимки без задержки затвора (сессия 14); `None` — бэкенд снимков не умеет.
    pub still: Option<StillDiagnostics>,
}

/// Бэкенд показа. Все методы вызываются из одного потока рендера (но сам объект может быть
/// создан в другом — отсюда `Send`).
pub trait DisplayBackend: Send + 'static {
    /// Принимает surface и создаёт цепочку показа заданного размера. При ошибке — см.
    /// [`AttachError`]: бэкенд ничего не удерживает.
    fn attach(&mut self, surface: NativeSurface, size: SurfaceSize) -> Result<(), AttachError>;

    /// Пересоздаёт цепочку под новый размер той же surface.
    fn resize(&mut self, size: SurfaceSize) -> Result<(), BackendError>;

    /// Рисует и показывает один кадр.
    fn render_and_present(&mut self, timing: FrameTiming) -> PresentOutcome;

    /// Дожидается GPU, уничтожает цепочку и возвращает surface. `None` — бэкенд ничего
    /// не держал (повторный вызов безопасен). После возврата окно можно отпускать.
    fn detach(&mut self) -> Option<NativeSurface>;

    /// Снимок диагностики (счётчики, параметры цепочки, период экрана); вызывается по таймеру.
    fn diagnostics(&mut self) -> BackendDiagnostics;

    /// Вычерпывает НОВЫЕ записи о реальных показах кадров (каждая выдаётся один раз).
    /// Дёшево, вызывается потоком на каждом кадре: драйвер хранит лишь короткую историю
    /// времён показа, и редкий опрос теряет записи. Пустой список — новых записей нет
    /// (или цепочки нет / расширения нет).
    fn drain_past_presentations(&mut self) -> Vec<PastPresentation>;

    /// Забирает замер CPU последнего `render_and_present` (каждый выдаётся один раз).
    /// `None` — бэкенд не меряет (fake, тестовый паттерн без камеры) или кадра не было.
    fn take_frame_cpu_timing(&mut self) -> Option<FrameCpuTiming> {
        None
    }

    /// Вычерпывает времена кадров на GPU, собранные с прошлого вызова (каждое выдаётся один раз,
    /// по возрастанию `frame_index`). Вызывается потоком на каждом кадре. Пустой список — новых
    /// замеров нет или бэкенд GPU не меряет (fake, тестовый паттерн): тогда планировщик берёт
    /// оценку GPU из конфига, а ADPF получает только CPU.
    fn drain_frame_gpu_timings(&mut self) -> Vec<FrameGpuTiming> {
        Vec::new()
    }

    /// Применяет команду видоискателю (между кадрами). По умолчанию видоискателя нет —
    /// `Unsupported` (тестовый паттерн, fake-бэкенды).
    fn apply_viewfinder_command(&mut self, command: ViewfinderCommand) -> ViewfinderCommandOutcome {
        let _ = command;
        ViewfinderCommandOutcome::Unsupported
    }

    /// Применяет команду UI-прохода (источник кадра UI поверх видоискателя; между кадрами). По умолчанию
    /// бэкенд UI рисовать не умеет — `Unsupported` (тестовый паттерн, fake-бэкенды); источник из `Install`
    /// при этом уничтожается.
    fn apply_ui_overlay_command(&mut self, command: UiOverlayCommand) -> UiOverlayOutcome {
        let _ = command;
        UiOverlayOutcome::Unsupported
    }

    /// Принимает команду снимка (между кадрами). По умолчанию снимков нет — команда возвращается потоку,
    /// и он завершает снимок честным `NotSecured` (тестовый паттерн, fake-бэкенды).
    fn apply_still_command(&mut self, command: StillCommand) -> StillCommandOutcome {
        StillCommandOutcome::unsupported(command)
    }

    /// Продвигает незаконченные снимки: опрос fence-ов, следующая полоса проявления, итоговые события.
    /// Поток зовёт на каждом показанном кадре после показа, а без показа (surface нет или пауза) — по таймеру,
    /// пока [`DisplayBackend::still_workload`] отвечает `Unfinished` (решение 150). Шаг не ждёт GPU: только опрос.
    /// По умолчанию снимков нет — ничего.
    fn advance_stills(&mut self) {}

    /// Есть ли снимки, ждущие итога. Поток рендера по этому ответу решает, будить ли себя без показа.
    /// Метод только читает состояние. По умолчанию снимков нет — `Idle`.
    fn still_workload(&self) -> StillWorkload {
        StillWorkload::Idle
    }

    /// Поток рендера неисправен: двигать снимки больше некому, каждый незаконченный снимок должен получить итог
    /// сейчас (захваченный — `Failed(Abandoned)`, не захваченный — `NotSecured`). Ожидание GPU ограничено
    /// реализацией. Это НЕ реакция на пропажу surface: без surface захваченные снимки доводятся до файла.
    /// По умолчанию снимков нет — ничего.
    fn abandon_stills(&mut self) {}

    /// Показ возобновлён после паузы (HOME и возврат): пока приложение было скрыто, система могла сбросить буферы
    /// окна, поэтому что сейчас на экране, неизвестно. Бэкенд, который пропускает тики без изменений (решение 214),
    /// обязан забыть «последний показанный кадр» и нарисовать следующий тик. По умолчанию — ничего.
    fn presentation_resumed(&mut self) {}
}
