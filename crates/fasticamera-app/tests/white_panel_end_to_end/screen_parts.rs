//! Подставные «границы процесса» экрана камеры для стенда панели белого: вибромотор, команды оболочки, служба
//! снимков и ящик команд рендеру. Всё, что в продукте живёт за границей процесса; сам экран, исполнитель эффектов,
//! порт к движку и источник фактов — настоящие.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use camera_screen::shutter::ShotRequest;
use fasticamera::camera_screen_capture::{CaptureService, SubmitOutcome};
use fasticamera::viewfinder_control::{ControlDelivery, RenderAdjustment, RenderControl};
use look_contract::LookRecipe;
use render_loop::ui_overlay::UiOverlayCommand;
use shell_contract::clock::MonotonicNanos;
use shell_contract::commands::{CommandOutcome, FrameRate, FrameRateOutcome, ScreenWake, ShellCommands};
use shell_contract::haptics::{HapticOutcome, HapticPattern, HapticRealizationKind, HapticsService};

/// Вибромотор, который помнит, что ему сыграли, и всегда «принимает» паттерн (исход `Queued`, как у телефона с мотором).
#[derive(Default)]
pub struct RecordingHaptics {
    played: Mutex<Vec<HapticPattern>>,
}

impl RecordingHaptics {
    /// Что сыграно, по порядку.
    pub fn played(&self) -> Vec<HapticPattern> {
        lock(&self.played).clone()
    }
}

impl HapticsService for RecordingHaptics {
    fn play(&self, pattern: HapticPattern) -> HapticOutcome {
        lock(&self.played).push(pattern);
        HapticOutcome::Queued(HapticRealizationKind::Primitive)
    }
}

/// Команды оболочки: принимаются и ничего не делают (в этих тестах оболочки нет).
pub struct QuietCommands;

impl ShellCommands for QuietCommands {
    fn request_camera_permission(&self) -> CommandOutcome {
        CommandOutcome::Queued
    }

    fn set_preferred_frame_rate(&self, _rate: FrameRate) -> FrameRateOutcome {
        FrameRateOutcome::Applied
    }

    fn open_system_gallery(&self) -> CommandOutcome {
        CommandOutcome::Queued
    }

    fn keep_screen_on(&self, _wake: ScreenWake) -> CommandOutcome {
        CommandOutcome::Queued
    }
}

/// Служба снимков: снимки в этих тестах не предмет проверки.
pub struct NoCapture;

impl CaptureService for NoCapture {
    fn submit(&self, _request: ShotRequest, _look: LookRecipe, _now: MonotonicNanos) -> SubmitOutcome {
        SubmitOutcome::Accepted
    }
}

/// Ящик команд рендеру: ЗАПОМИНАЕТ их и НЕ применяет. Эталон теста — чистый кадр видоискателя, поэтому «место
/// кадра», вид и приборы, которые экран просит у рендера, к бэкенду стенда не доходят (стенд проверяет путь
/// «палец → движок → пиксели», а не путь «экран → рендер»).
#[derive(Default)]
pub struct RecordingRender {
    adjustments: Mutex<Vec<RenderAdjustment>>,
}

impl RecordingRender {
    /// Сколько регулировок рендеру экран успел попросить.
    pub fn adjustments_requested(&self) -> usize {
        lock(&self.adjustments).len()
    }
}

impl RenderControl for RecordingRender {
    fn adjust(&self, adjustment: RenderAdjustment) -> ControlDelivery {
        lock(&self.adjustments).push(adjustment);
        ControlDelivery::Delivered
    }

    fn ui_overlay(&self, _command: UiOverlayCommand) -> ControlDelivery {
        ControlDelivery::Delivered
    }
}

/// Замок без паники на отравлении: тест всё равно упадёт на своей проверке.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Обёртка, чтобы стенд хранил одну и ту же запись и как `Arc<dyn …>` для исполнителя, и как конкретный тип.
pub struct ScreenSideRecords {
    pub haptics: Arc<RecordingHaptics>,
    pub render: Arc<RecordingRender>,
}

impl ScreenSideRecords {
    pub fn new() -> Self {
        Self {
            haptics: Arc::new(RecordingHaptics::default()),
            render: Arc::new(RecordingRender::default()),
        }
    }
}
