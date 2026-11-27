//! Исполнитель эффектов экрана камеры (сессия 12): превращает намерения [`Effect`] в вызовы сервисов.
//!
//! Экран ничего не вызывает сам — он отдаёт `Effect`, а этот модуль знает, кому они адресованы:
//! * `Camera` → ручка движка ([`CameraIntentPort`]);
//! * `Haptic` → [`HapticsService`];
//! * `OpenSystemGallery` → [`ShellCommands::open_system_gallery`];
//! * `Lens` → ящик намерений объектива [`LensIntentChannel`] (сессия 13): «последнее побеждает», НЕ очередь команд
//!   движка — промежуточные значения колеса и щипка не копятся;
//! * `Viewfinder` → команда рендеру `SetFraming` через [`RenderControl`] (решение 93: место кадра решает экран);
//! * `Capture` → [`CaptureService`] (фейк до сессии 14), запрос передаётся целиком (с моментом нажатия);
//! * `Look` → [`LookEffects`] (отдельный модуль): библиотека пресетов, команда `SetLook` рендеру, запись файла
//!   отдельным потоком; запрос снимка (`Capture`) получает значения вида НА МОМЕНТ обработки эффекта;
//! * `PlayShutterSound` → [`ShutterSoundService`]: отказ (`NotAttached`, `Backpressure`) — счётчик, не ошибка экрана.
//!
//! Исполнитель живёт на ПОТОКЕ РЕНДЕРА (внутри источника UI) и ничего не ждёт: всё — постановка в ограниченные
//! очереди. Желаемая компенсация экспозиции — исключение из «потеряно навсегда»: экран уже показывает новое EV,
//! поэтому при `QueueFull` последнее значение досылается на следующих кадрах, пока не примется (последнее
//! побеждает; [`EffectExecutor::resend_pending_exposure_bias`]). Фокус и блокировка не досылаются: к моменту
//! повтора они устарели бы (точку уже сменили или отпустили). Любой отказ — счётчик по виду ([`EffectCounters`]), не паника и не тишина. Запрос снимка, который
//! сервис не принял, превращается в обратное действие экрану `CaptureFinished(Failed)`: иначе затвор остался бы
//! в «снимаю» навсегда ([`EffectExecutor::execute`] возвращает такие действия вызывающему).

use std::sync::Arc;

use camera_contract::auto_exposure::SignalDelivery;
use camera_contract::control::ExposureBias;
use camera_contract::white::WhiteTransition;
use camera_contract::zoom::{LensIntent, LensIntentChannel, StandbyDemand, ZoomGesture};
use camera_core::white::WhiteFollowPause;
use camera_core::{CameraHandle, CommandId, CommandRejected};
use camera_screen::shutter::{CaptureFailureReason, CaptureOutcome, ShotRequest};
use camera_screen::{Effect, FramingIntent, ScreenAction, ScreenCameraIntent, WhiteTrackingHold};
use look_contract::LookIntent;
use render_loop::viewfinder::{
    FramingCrop, FramingDestination, NormalizedScreenRect, ViewfinderCommand, ViewfinderFraming,
};
use shell_contract::clock::MonotonicNanos;
use shell_contract::commands::{CommandOutcome, ShellCommands};
use shell_contract::haptics::{HapticOutcome, HapticPattern, HapticsService};
use shell_contract::still_media::ShutterSoundService;
use shell_contract::text_input::TextPurpose;

use crate::camera_screen_capture::{CaptureService, SubmitOutcome, SubmitRejection};
use crate::camera_white_effects::{ManualWhiteEdit, PendingWhite, WhiteEffects, WhiteIntentCounters};
use crate::camera_white_hold::{WhiteHoldCounters, WhiteHoldLink};
use crate::look_effects::{LookCounters, LookEffects};
use crate::look_service::LookHandle;
use crate::scope_effects::{ScopeCounters, ScopeEffects};
use crate::text_effects::{TextEffects, TextRequestCounters};
use crate::text_input_broker::TextInputBroker;
use crate::viewfinder_control::{ControlDelivery, RenderAdjustment, RenderControl};

const LOG_TARGET: &str = "fasticamera::camera_screen";

/// Итог доставки намерения камере. Не `Result`: «движка нет» — не ошибка очереди.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraIntentOutcome {
    /// Команда принята очередью движка.
    Sent(CommandId),
    /// Очередь движка полна.
    QueueFull,
    /// Поток движка остановлен.
    EngineStopped,
    /// Движка камеры в приложении нет (не поднялся).
    NoEngine,
}

/// Куда экран шлёт намерения камере. Узкий контракт: исполнитель не знает про `CameraHandle`.
pub trait CameraIntentPort: Send {
    fn deliver(&self, intent: ScreenCameraIntent) -> CameraIntentOutcome;
}

/// Порт на ручку настоящего движка: имена намерений совпадают с методами `CameraHandle`.
pub struct HandleCameraPort {
    handle: CameraHandle,
}

impl HandleCameraPort {
    pub fn new(handle: CameraHandle) -> Self {
        Self { handle }
    }
}

impl CameraIntentPort for HandleCameraPort {
    fn deliver(&self, intent: ScreenCameraIntent) -> CameraIntentOutcome {
        let sent = match intent {
            ScreenCameraIntent::FocusAndExposeAt(point) => self.handle.focus_and_expose_at(point),
            ScreenCameraIntent::LockFocusAndExposureAt(point) => self.handle.lock_focus_and_exposure_at(point),
            ScreenCameraIntent::ResetPointOfInterest => self.handle.reset_point_of_interest(),
            ScreenCameraIntent::SetExposureBias(bias) => self.handle.set_exposure_bias(bias),
            // Белое ручками панели всегда мгновенное: шкала должна идти за пальцем без плавного перехода (WB17).
            ScreenCameraIntent::SetWhite(setting) => self.handle.set_white(setting, WhiteTransition::Immediate),
            ScreenCameraIntent::AdjustWhite(adjustment) => {
                self.handle.adjust_white(adjustment, WhiteTransition::Immediate)
            }
            ScreenCameraIntent::TakeWhite(source) => self.handle.take_white(source, WhiteTransition::Immediate),
            ScreenCameraIntent::FollowWhite(source) => self.handle.follow_white(source),
            ScreenCameraIntent::HoldWhiteTracking(hold) => self.handle.set_white_follow_pause(match hold {
                WhiteTrackingHold::Running => WhiteFollowPause::Running,
                WhiteTrackingHold::PausedForCalibration => WhiteFollowPause::Paused,
            }),
        };
        match sent {
            Ok(command) => CameraIntentOutcome::Sent(command),
            Err(CommandRejected::QueueFull) => CameraIntentOutcome::QueueFull,
            Err(CommandRejected::EngineStopped) => CameraIntentOutcome::EngineStopped,
        }
    }
}

/// Порт, когда движка нет: каждое намерение — честное `NoEngine`.
pub struct AbsentCameraPort;

impl CameraIntentPort for AbsentCameraPort {
    fn deliver(&self, _intent: ScreenCameraIntent) -> CameraIntentOutcome {
        CameraIntentOutcome::NoEngine
    }
}

/// Счётчики намерений камере.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CameraIntentCounters {
    pub sent: u64,
    pub queue_full: u64,
    pub engine_stopped: u64,
    pub no_engine: u64,
    /// Досылки желаемого EV, принятые очередью движка (в `sent` не входят: это повтор, а не новое намерение).
    pub exposure_resends_accepted: u64,
    /// Досылки, снова упёршиеся в полную очередь (значение остаётся ждать).
    pub exposure_resends_queue_full: u64,
    /// Досылки, от которых отказались навсегда: движок остановлен или его нет.
    pub exposure_resends_abandoned: u64,
}

/// Сколько последних времён принятых вибраций помнит [`RecentHapticTimes`] (с запасом на 30 рисок колеса за жест).
pub const RECENT_HAPTICS_CAPACITY: usize = 32;

/// Времена кадров (шкала часов потока рендера, нс), на которых принята последняя вибрация, — кольцо фиксированной
/// длины. Нужно сценарию, чтобы судить «промежуток между вибрациями» ОКНОМ одного шага, а не наименьшим за всю жизнь
/// процесса (одна пара в одном кадре до сценария обнулила бы такой минимум навсегда). Копируемое: живёт в счётчиках.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecentHapticTimes {
    nanos: [i64; RECENT_HAPTICS_CAPACITY],
    /// Сколько вибраций принято за всё время (больше ёмкости — старые вытеснены).
    total: u64,
}

impl Default for RecentHapticTimes {
    fn default() -> Self {
        Self {
            nanos: [0; RECENT_HAPTICS_CAPACITY],
            total: 0,
        }
    }
}

impl RecentHapticTimes {
    fn push(&mut self, at: MonotonicNanos) {
        let slot = (self.total % RECENT_HAPTICS_CAPACITY as u64) as usize;
        self.nanos[slot] = at.as_nanos();
        self.total += 1;
    }

    /// Кольцо с заданными временами (для тестов потребителей кольца: сценарий `zoom`).
    #[cfg(test)]
    pub(crate) fn from_times(times: &[MonotonicNanos]) -> Self {
        let mut recent = Self::default();
        for time in times {
            recent.push(*time);
        }
        recent
    }

    /// Сколько вибраций принято за всё время.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// Времена принятых вибраций, которые ещё помнит кольцо, от старых к новым.
    pub fn retained(&self) -> Vec<MonotonicNanos> {
        let kept = self.total.min(RECENT_HAPTICS_CAPACITY as u64);
        (self.total - kept..self.total)
            .map(|index| MonotonicNanos::from_nanos(self.nanos[(index % RECENT_HAPTICS_CAPACITY as u64) as usize]))
            .collect()
    }
}

/// Счётчики хаптики по видам итога.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HapticCounters {
    pub queued: u64,
    pub unsupported: u64,
    pub not_attached: u64,
    pub backpressure: u64,
    /// Когда принимались последние вибрации (метки — время кадра UI, на котором исполнитель принял вибрацию).
    /// Быстрая прокрутка колеса зума не должна «смазывать» мотор в гул (решение 112): ограничитель частоты стоит в
    /// виджете и решает по ВРЕМЕНИ СОБЫТИЙ КАСАНИЯ, а не кадра UI — это другие часы, и два принятых тика могут получить
    /// метки одного кадра. Поэтому по этим меткам промежутки НЕ судят (сценарий `zoom` сверяет только число
    /// вибраций); интервал ограничителя доказывают юнит-тесты `ui-kit` на времени касаний.
    pub recent_queued: RecentHapticTimes,
}

/// Счётчики намерений объектива (сессия 13): ящик «последнее побеждает».
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LensCounters {
    /// Намерений положено в ящик (в том числе заменивших ещё не прочитанное движком).
    pub published: u64,
    /// Из них при публикации движок уже был привязан и получил толчок.
    pub wakes_delivered: u64,
    /// Из них движок ещё не привязан: толчок пропущен, `bind` досылает его сам.
    pub wakes_before_engine_bound: u64,
}

/// Счётчики команд оболочке (галерея).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShellCommandCounters {
    pub queued: u64,
    pub not_attached: u64,
    pub backpressure: u64,
}

/// Счётчики команд рендеру «место видоискателя».
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FramingCounters {
    /// Команда лежит в ящике потока рендера.
    pub delivered: u64,
    /// Поток рендера уже завершён.
    pub render_gone: u64,
    /// Рендер не поднимался.
    pub render_unavailable: u64,
    /// Прямоугольник экрана не прошёл проверку типа рендера (команда не отправлена).
    pub invalid_rect: u64,
}

/// Счётчики запросов снимка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CaptureCounters {
    pub accepted: u64,
    pub rejected_busy: u64,
    pub rejected_unavailable: u64,
}

/// Счётчики звука затвора: итог каждой попытки по виду (звук — украшение, отказ не ломает снимок).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShutterSoundCounters {
    /// Команда поставлена в очередь платформы.
    pub queued: u64,
    /// Платформа не подключена (Activity нет): играть некому.
    pub not_attached: u64,
    /// Очередь команд платформы переполнена: звук отброшен.
    pub backpressure: u64,
}

/// Все счётчики исполнителя.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EffectCounters {
    pub camera: CameraIntentCounters,
    pub haptics: HapticCounters,
    pub gallery: ShellCommandCounters,
    pub framing: FramingCounters,
    pub capture: CaptureCounters,
    pub shutter_sound: ShutterSoundCounters,
    pub lens: LensCounters,
    /// Вид картинки и пресеты (решение 178): отказы библиотеки, доставка рендеру, запись файла.
    pub look: LookCounters,
    /// Цветовые приборы (14h): доставка намерений рендеру.
    pub scopes: ScopeCounters,
    /// Просьбы экрана спросить имя пресета (решение 197): исходы по видам.
    pub text: TextRequestCounters,
    /// Ручное белое и команда «взять» (white-balance 02): принятое, отказы, досылки, недоставленные команды.
    pub white: WhiteIntentCounters,
    /// Пауза слежения за белым на время раздела калибровки: принятое и отказы (решение 289).
    pub white_hold: WhiteHoldCounters,
}

/// Сервисы, которым адресованы эффекты (собираются композицией один раз на экран).
pub struct EffectTargets {
    pub camera: Box<dyn CameraIntentPort>,
    pub haptics: Arc<dyn HapticsService>,
    pub commands: Arc<dyn ShellCommands>,
    pub render: Arc<dyn RenderControl>,
    pub capture: Arc<dyn CaptureService>,
    /// Звук затвора (`MediaActionSound` на телефоне): эффект играет его в момент снимка.
    pub shutter_sound: Arc<dyn ShutterSoundService>,
    /// Ящик намерений объектива: тот же клон, что у движка камеры (`CameraServices::lens_intents`).
    pub lens: LensIntentChannel,
    /// Библиотека пресетов вида: общая на процесс ручка, а не копия экрана (экран переставляется, пресеты живут).
    /// `None` — в сборке нет хранилища пресетов (ПК-сценарии, тесты): намерения по виду считаются и отбрасываются.
    pub look: Option<LookHandle>,
}

/// Исполнитель эффектов.
pub struct EffectExecutor {
    targets: EffectTargets,
    /// Намерения по виду картинки: библиотека пресетов, команда рендеру, запись файла. Счётчики — свои, в
    /// [`EffectCounters::look`] они подставляются при чтении.
    look: LookEffects,
    /// Намерения по цветовым приборам: команда рендеру и счётчики её доставки.
    scopes: ScopeEffects,
    /// Просьбы «спроси имя»: брокер запросов текста и счётчики исходов.
    text: TextEffects,
    counters: EffectCounters,
    /// Время показа текущего кадра UI (часы потока рендера): им фейк-сервис снимка отсчитывает задержку ответа.
    frame_time: MonotonicNanos,
    /// Последнее желаемое EV, которое камера ещё не приняла из-за полной очереди. Новое значение заменяет старое.
    pending_exposure_bias: Option<ExposureBias>,
    /// Ручное белое: отложенные правки и счётчики (см. [`WhiteEffects`]).
    white: WhiteEffects,
    /// Пауза слежения за белым: желаемое против принятого (см. [`WhiteHoldLink`]).
    white_hold: WhiteHoldLink,
}

/// Конец жизни исполнителя = конец жизни экрана: источник кадров снимают сценарий, рендер или переустановка, и
/// все пути заканчиваются тем, что исполнитель уничтожается на потоке рендера **после** последнего исполненного
/// кадра. Это единственная точка, где гарантированно нет гонки с ещё идущим кадром (команда «снять экран»
/// доезжает до рендера асинхронно): последним в ящик попадёт «тёплый сосед не нужен», и камера не останется
/// греться из-за исчезнувшего колеса (решение 117: спрос живёт, пока колесо или щипок на экране).
impl Drop for EffectExecutor {
    fn drop(&mut self) {
        self.release_standby_demand();
    }
}

impl EffectExecutor {
    pub fn new(targets: EffectTargets) -> Self {
        Self {
            look: LookEffects::new(targets.look.clone()),
            scopes: ScopeEffects::default(),
            text: TextEffects::default(),
            targets,
            counters: EffectCounters::default(),
            frame_time: MonotonicNanos::from_nanos(0),
            pending_exposure_bias: None,
            white: WhiteEffects::default(),
            white_hold: WhiteHoldLink::default(),
        }
    }

    /// Подключает брокер запросов текста (решение 197): без него просьба экрана «спроси имя» получает
    /// `TextRequestNotSent`. Брокер общий на процесс, как библиотека пресетов; приёмник его ответов ставит сервис экрана.
    #[must_use]
    pub fn with_text_input(mut self, broker: TextInputBroker) -> Self {
        self.text = TextEffects::new(Some(broker));
        self
    }

    /// Экран только что поставлен на рендер: показ вида приводится к действующему рецепту библиотеки (см.
    /// [`LookEffects::release_clean_view_on_install`]). Зовёт источник UI на ПЕРВОМ кадре, уже на потоке рендера:
    /// команда идёт после установки источника, а не до неё.
    pub fn on_screen_installed(&mut self) {
        self.look.release_clean_view_on_install(self.targets.render.as_ref());
    }

    /// Каждый кадр: если рендеру не удалось доставить вид (см. [`LookEffects::retry_if_render_unsure`]), повторяет.
    /// Когда всё доставлено, ничего не шлёт и почти ничего не стоит.
    pub fn retry_look_render_if_unsure(&mut self) {
        self.look.retry_if_render_unsure(self.targets.render.as_ref());
    }

    /// Начало кадра UI: запоминает время показа и даёт сервису снимка выдать созревшие ответы.
    /// Вызывается ДО разбора входа кадра, чтобы ответ `CaptureFinished` попал в этот же кадр.
    pub fn begin_frame(&mut self, now: MonotonicNanos) {
        self.frame_time = now;
        self.targets.capture.poll_completions(now);
        // Итоги записи файла пресетов: считаем и громко логируем сбои, рендер диск не ждёт.
        self.look.poll_save_outcomes();
    }

    /// Последняя проблема вида (отказ библиотеки, сбой записи) для фактов экрана; `None` — проблем не было.
    pub fn latest_look_problem(&self) -> Option<look_contract::LookProblem> {
        self.look.latest_problem()
    }

    /// Ждёт ли желаемое EV досылки (диагностика и тесты).
    pub fn pending_exposure_bias(&self) -> Option<ExposureBias> {
        self.pending_exposure_bias
    }

    /// Досылает последнее желаемое EV, не принятое из-за полной очереди. Вызывается на каждом кадре ДО новых
    /// эффектов, поэтому более свежее значение из этого же кадра всегда идёт позже и побеждает.
    pub fn resend_pending_exposure_bias(&mut self) {
        let Some(bias) = self.pending_exposure_bias else {
            return;
        };
        let counters = &mut self.counters.camera;
        match self.targets.camera.deliver(ScreenCameraIntent::SetExposureBias(bias)) {
            CameraIntentOutcome::Sent(_) => {
                counters.exposure_resends_accepted += 1;
                self.pending_exposure_bias = None;
            }
            // Очередь всё ещё полна: значение остаётся ждать следующего кадра.
            CameraIntentOutcome::QueueFull => counters.exposure_resends_queue_full += 1,
            // Движка нет и не будет: ждать бессмысленно, но отказ не тихий.
            CameraIntentOutcome::EngineStopped | CameraIntentOutcome::NoEngine => {
                counters.exposure_resends_abandoned += 1;
                self.pending_exposure_bias = None;
                tracing::warn!(target: LOG_TARGET, ?bias, "движка камеры нет: досылка EV отменена");
            }
        }
    }

    /// Отложенные ручные правки белого, которые движок ещё не принял (диагностика и тесты).
    pub fn pending_white(&self) -> PendingWhite {
        self.white.pending()
    }

    /// Досылает ручное белое, не принятое из-за полной очереди. Как и EV, зовётся каждый кадр ДО новых эффектов.
    pub fn resend_pending_white(&mut self) {
        self.white.resend_pending(self.targets.camera.as_ref());
    }

    /// Каждый кадр: приводит паузу слежения за белым в движке к желанию экрана (см. [`WhiteHoldLink`]). Зовётся
    /// даже когда эффектов нет: уровень сверяется независимо от действий человека.
    pub fn drive_white_tracking_hold(&mut self, desired: WhiteTrackingHold) {
        self.white_hold.drive(desired, self.targets.camera.as_ref());
    }

    /// Что движок принял последним по паузе слежения (диагностика и тесты); `None` — ещё ничего.
    pub fn acknowledged_white_tracking_hold(&self) -> Option<WhiteTrackingHold> {
        self.white_hold.acknowledged()
    }

    pub fn counters(&self) -> EffectCounters {
        EffectCounters {
            white: self.white.counters(),
            white_hold: self.white_hold.counters(),
            look: self.look.counters(),
            scopes: self.scopes.counters(),
            text: self.text.counters(),
            ..self.counters
        }
    }

    /// Исполняет эффекты по порядку. Возвращает действия, которые нужно вернуть экрану (отвергнутый снимок).
    pub fn execute(&mut self, effects: Vec<Effect>) -> Vec<ScreenAction> {
        let mut feedback = Vec::new();
        for effect in effects {
            match effect {
                Effect::Camera(intent) => feedback.extend(self.deliver_camera_intent(intent)),
                Effect::Haptic(pattern) => self.play_haptic(pattern),
                Effect::OpenSystemGallery => self.open_gallery(),
                Effect::Viewfinder(intent) => self.place_viewfinder(intent),
                Effect::Lens(intent) => self.publish_lens_intent(intent),
                Effect::Look(intent) => self.run_look_intent(intent),
                Effect::Scopes(intent) => self.scopes.run_intent(intent, self.targets.render.as_ref()),
                Effect::Capture(request) => feedback.extend(self.submit_capture(request)),
                Effect::PlayShutterSound => self.play_shutter_sound(),
                Effect::CleanView(view) => self.look.set_clean_view(view, self.targets.render.as_ref()),
                Effect::RequestText { purpose, initial_text } => {
                    feedback.push(self.request_text(purpose, initial_text));
                }
            }
        }
        feedback
    }

    /// Просьба экрана «спроси имя»: исход возвращается экрану обратным действием (номер диалога или «не отправлено»).
    fn request_text(&mut self, purpose: TextPurpose, initial_text: String) -> ScreenAction {
        self.text.request(purpose, initial_text)
    }

    /// Белые намерения идут в [`WhiteEffects`] (у них своя досылка); остальные — по общему пути. Возвращает
    /// обратное действие экрану, если намерение не дошло и человеку надо это показать.
    fn deliver_camera_intent(&mut self, intent: ScreenCameraIntent) -> Option<ScreenAction> {
        let camera = self.targets.camera.as_ref();
        match intent {
            ScreenCameraIntent::SetWhite(setting) => {
                self.white.run_manual_edit(camera, ManualWhiteEdit::Whole(setting));
                None
            }
            ScreenCameraIntent::AdjustWhite(adjustment) => {
                self.white.run_manual_edit(camera, ManualWhiteEdit::Axis(adjustment));
                None
            }
            ScreenCameraIntent::TakeWhite(source) => self.white.run_take(camera, source),
            ScreenCameraIntent::FollowWhite(source) => self.white.run_follow(camera, source),
            // Экран эту команду эффектом не шлёт (она — уровень, `drive_white_tracking_hold`), но если пришла, идёт по
            // тому же пути «желаемое против принятого», а не теряется.
            ScreenCameraIntent::HoldWhiteTracking(hold) => {
                self.white_hold.drive(hold, camera);
                None
            }
            ScreenCameraIntent::FocusAndExposeAt(_)
            | ScreenCameraIntent::LockFocusAndExposureAt(_)
            | ScreenCameraIntent::ResetPointOfInterest
            | ScreenCameraIntent::SetExposureBias(_) => {
                self.deliver_point_or_exposure_intent(intent);
                None
            }
        }
    }

    fn deliver_point_or_exposure_intent(&mut self, intent: ScreenCameraIntent) {
        let counters = &mut self.counters.camera;
        // Только EV досылается: остальные намерения при отказе устаревают (см. доку модуля).
        let exposure_bias = match intent {
            ScreenCameraIntent::SetExposureBias(bias) => Some(bias),
            ScreenCameraIntent::FocusAndExposeAt(_)
            | ScreenCameraIntent::LockFocusAndExposureAt(_)
            | ScreenCameraIntent::ResetPointOfInterest
            // Белые намерения сюда не доходят: их исполняет `WhiteEffects` (см. `deliver_camera_intent`).
            | ScreenCameraIntent::SetWhite(_)
            | ScreenCameraIntent::AdjustWhite(_)
            | ScreenCameraIntent::TakeWhite(_)
            | ScreenCameraIntent::FollowWhite(_)
            // Паузу слежения ведёт `WhiteHoldLink` по уровню (`drive_white_tracking_hold`), не по эффекту.
            | ScreenCameraIntent::HoldWhiteTracking(_) => None,
        };
        match self.targets.camera.deliver(intent) {
            CameraIntentOutcome::Sent(_) => {
                counters.sent += 1;
                // Свежее значение принято: ждавшее старое устарело.
                if exposure_bias.is_some() {
                    self.pending_exposure_bias = None;
                }
            }
            CameraIntentOutcome::QueueFull => {
                counters.queue_full += 1;
                if let Some(bias) = exposure_bias {
                    self.pending_exposure_bias = Some(bias);
                    tracing::warn!(target: LOG_TARGET, ?intent, "очередь движка полна: EV будет досылаться");
                } else {
                    tracing::warn!(target: LOG_TARGET, ?intent, "очередь движка полна: намерение экрана потеряно");
                }
            }
            CameraIntentOutcome::EngineStopped => {
                counters.engine_stopped += 1;
                if exposure_bias.is_some() {
                    self.pending_exposure_bias = None;
                }
                tracing::warn!(target: LOG_TARGET, ?intent, "движок камеры остановлен: намерение экрана потеряно");
            }
            CameraIntentOutcome::NoEngine => {
                counters.no_engine += 1;
                if exposure_bias.is_some() {
                    self.pending_exposure_bias = None;
                }
            }
        }
    }

    /// Намерение по виду: библиотека применяет, рендеру уходит новый вид, файл просится на запись.
    fn run_look_intent(&mut self, intent: LookIntent) {
        self.look.run_intent(intent, self.targets.render.as_ref());
    }

    fn play_haptic(&mut self, pattern: HapticPattern) {
        let counters = &mut self.counters.haptics;
        match self.targets.haptics.play(pattern) {
            HapticOutcome::Queued(_) => {
                counters.queued += 1;
                counters.recent_queued.push(self.frame_time);
            }
            HapticOutcome::Unsupported => counters.unsupported += 1,
            HapticOutcome::NotAttached => counters.not_attached += 1,
            HapticOutcome::Backpressure => counters.backpressure += 1,
        }
    }

    fn open_gallery(&mut self) {
        let counters = &mut self.counters.gallery;
        match self.targets.commands.open_system_gallery() {
            CommandOutcome::Queued => counters.queued += 1,
            CommandOutcome::NotAttached => counters.not_attached += 1,
            CommandOutcome::Backpressure => counters.backpressure += 1,
        }
    }

    /// Намерение объектива — в ящик «последнее побеждает» (не в очередь команд движка, не может быть `QueueFull`).
    fn publish_lens_intent(&mut self, intent: LensIntent) {
        let counters = &mut self.counters.lens;
        let (_generation, delivery) = self.targets.lens.publish(intent);
        counters.published += 1;
        match delivery {
            SignalDelivery::Delivered => counters.wakes_delivered += 1,
            SignalDelivery::NotBoundYet => counters.wakes_before_engine_bound += 1,
        }
    }

    /// Снять спрос на тёплого соседа от этого экрана: если в ящике лежит `KeepNeighbourWarm`, кладёт то же
    /// намерение (направление, коэффициент, жест) с `NotNeeded`. Ничего не делает, если ящик пуст или спроса нет:
    /// чужое намерение (сценарий) без спроса не трогаем и пустых толчков движку не шлём.
    /// Возвращает `true`, если «не нужно» опубликовано.
    pub fn release_standby_demand(&mut self) -> bool {
        let Some((_generation, latest)) = self.targets.lens.latest() else {
            return false;
        };
        if latest.standby != StandbyDemand::KeepNeighbourWarm {
            return false;
        }
        // Жест — всегда `Continuous`, а не повтор последнего: повтор `Discrete` имеет побочный эффект
        // (снимает блокировку объектива после отказа), а здесь мы только снимаем спрос (ревью 13b, замечание 6).
        let release = LensIntent {
            gesture: ZoomGesture::Continuous,
            ..latest
        };
        self.publish_lens_intent(release.with_standby(StandbyDemand::NotNeeded));
        true
    }

    fn place_viewfinder(&mut self, intent: FramingIntent) {
        let counters = &mut self.counters.framing;
        let framing = match framing_of(&intent) {
            Ok(framing) => framing,
            Err(error) => {
                counters.invalid_rect += 1;
                tracing::error!(target: LOG_TARGET, %error, destination = ?intent.destination, "место видоискателя от экрана неверно: команда не отправлена");
                return;
            }
        };
        // Напрямую через `adjust`, а не через `*_for_scenario`: место кадра — настройка продукта, а не сценария,
        // конец сценария её не откатывает (решение 93: место живёт в бэкенде рендера).
        let delivery = self
            .targets
            .render
            .adjust(RenderAdjustment::Viewfinder(ViewfinderCommand::SetFraming(framing)));
        match delivery {
            ControlDelivery::Delivered => counters.delivered += 1,
            ControlDelivery::RenderGone => counters.render_gone += 1,
            ControlDelivery::RenderUnavailable { .. } => counters.render_unavailable += 1,
        }
    }

    /// Звук затвора. Платформа может отказать — это счётчик, а не ошибка экрана: снимок от звука не зависит.
    fn play_shutter_sound(&mut self) {
        let counters = &mut self.counters.shutter_sound;
        match self.targets.shutter_sound.play_shutter_sound() {
            CommandOutcome::Queued => counters.queued += 1,
            CommandOutcome::NotAttached => counters.not_attached += 1,
            CommandOutcome::Backpressure => counters.backpressure += 1,
        }
    }

    /// Запрос снимка (целиком, с моментом нажатия); отвергнутый — действие `CaptureFinished(Failed)` обратно экрану.
    fn submit_capture(&mut self, request: ShotRequest) -> Option<ScreenAction> {
        let counters = &mut self.counters.capture;
        let request_id = request.request;
        // Вид — значения на момент нажатия, а не ссылка на пресет (инвариант 3): намерения вида, обработанные раньше
        // в этом же кадре, уже учтены, а поздние запрос не меняют.
        let look = self.look.recipe_for_still(self.targets.render.as_ref());
        let rejection = match self.targets.capture.submit(request, look, self.frame_time) {
            SubmitOutcome::Accepted => {
                counters.accepted += 1;
                return None;
            }
            SubmitOutcome::Rejected(rejection) => rejection,
        };
        let reason = match rejection {
            SubmitRejection::Busy => {
                counters.rejected_busy += 1;
                CaptureFailureReason::Processing
            }
            SubmitRejection::ServiceUnavailable => {
                counters.rejected_unavailable += 1;
                CaptureFailureReason::Camera
            }
        };
        tracing::warn!(target: LOG_TARGET, ?rejection, "сервис снимка отверг запрос: экрану возвращён провал");
        Some(ScreenAction::CaptureFinished {
            request: request_id,
            outcome: CaptureOutcome::Failed(reason),
        })
    }
}

/// Намерение экрана → команда рендеру. У экрана свой тип прямоугольника, у рендера — проверяемый: перевод с ошибкой.
///
/// Кадрирование — `FramingCrop::Zoom`: рендер считает его по камере КАЖДОГО кадра одной формулой с экраном.
fn framing_of(intent: &FramingIntent) -> Result<ViewfinderFraming, render_loop::viewfinder::ScreenRectError> {
    let rect = intent.destination;
    Ok(ViewfinderFraming {
        crop: FramingCrop::Zoom(intent.zoom.clone()),
        destination: FramingDestination::ScreenRect(NormalizedScreenRect::new(
            rect.left,
            rect.top,
            rect.width,
            rect.height,
        )?),
    })
}

#[cfg(test)]
#[path = "camera_screen_effects_tests.rs"]
mod tests;
