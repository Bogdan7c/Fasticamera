//! Непрерывность картинки видоискателя при смене потока камеры (сессия 13, решения 110 и 113: «0 чёрных
//! кадров»; задел V7 — будущей записи нужен непрерывный ряд кадров).
//!
//! # Что это
//! Чистая (без GPU) машина состояний `Live → Holding → Live`. Пока камера переключается (старый поток
//! закрылся, новый ещё не отдал показываемых кадров), на экране остаётся ПОСЛЕДНИЙ показанный кадр.
//! Картинка этого кадра живёт отдельным изображением (`pipelines::held`), машина только решает, что
//! показывать в каждом кадре, и считает итоги.
//!
//! # Как работает один кадр показа
//! 1. [`ContinuityMachine::plan`] — решение ДО записи команд: что положить на экран ([`Screen`]), надо ли
//!    начать удержание (отцепить картинку у ISP ДО записи кадра нового потока) и закончить ли его.
//!    Решает по событию потока ([`StreamEvent`]), времени показа кадра и состоянию машины; ничего не меняет.
//! 2. Запись команд (владелец GPU — `CameraSession`/`CameraPipelines`).
//! 3. [`ContinuityMachine::commit`] — итог ПО ФАКТУ записанного ([`Composed`]): машина меняет состояние по
//!    тому, что реально попало на экран, а не по намерению. Поэтому «ни одного чёрного кадра» —
//!    проверяемое утверждение: чёрный кадр, записанный вместо удержанного, виден в счётчиках.
//!
//! # Условие показа нового потока (reveal)
//! Удержание заканчивается на первом ОБРАБОТАННОМ кадре нового потока, у которого (а) известен цвет (AWB не
//! скрывает кадр) и (б) яркость правдоподобна (`ExposureReadiness::Ready`: наш AE измерил сцену и показанный
//! уровень отличается от цели не больше допуска; если AE нечего измерять, ожидание не включается — см.
//! `ExposureRunner::reveal_readiness`), ИЛИ прошло `max_hold_ms` — тогда новый кадр показывается как есть
//! (удержание не длится вечно). Если нового потока нет вовсе `max_hold_without_stream_ms` — честный чёрный:
//! «камера недоступна» показывает экран, а не замороженная картинка. Любой ПРИХОД кадра нового потока (даже
//! необработанного: нет параметров ISP, геометрия не подошла, импорт не удался — [`StreamEvent::Unpresentable`])
//! считается жизнью потока и этот срок откладывает: камера-то стримит. Но если показать нечего и после
//! `max_hold_ms` (постоянный отказ ISP), удержание тоже кончается честным чёрным
//! ([`HoldEnd::NoPresentableFrame`]): замороженная картинка не живёт вечно.
//!
//! # Время
//! Только время показа кадра (`MonotonicNanos`, шкала vsync), никаких `Instant::now()`: тесты ведут
//! часы сами.

use camera_contract::ids::LensFacing;
use camera_contract::zoom::{PreviewMirror, ZoomRatio};
use shell_contract::clock::MonotonicNanos;
use ui_draw::frame_source::BackdropContinuity;

use super::white_balance::AwbStream;
use crate::config::ContinuityConfig;
use crate::viewfinder::continuity::{BlackKind, FrameLabel, HoldEnd, HoldReason, TransitionKind};
use crate::viewfinder::framing::WantedZoom;

/// Нс в мс.
const NANOS_PER_MILLI: i64 = 1_000_000;

/// Что известно о показанной картинке (она же — удержанная после начала удержания).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PictureMeta {
    /// Поток, чей кадр на картинке.
    pub stream: AwbStream,
    /// Коэффициент шкалы зума, который показывает картинка (`FramedCrop::shown_ratio`).
    pub shown_ratio: ZoomRatio,
    /// Зеркалилась ли картинка при показе (удержанный кадр остаётся зеркальным, как был).
    pub mirror: PreviewMirror,
    /// Направление камеры кадра (`None` — камеры нет в каталоге зума или кадрирование фиксированное).
    pub facing: Option<LensFacing>,
}

/// Что произошло с потоком в этом кадре показа (по опросу видоискателя и подготовке кадра).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StreamEvent {
    /// Кадра нового потока в этом тике нет, поток не объявлен закрытым.
    Quiet,
    /// Кадр нового потока ПРИШЁЛ, но показать его нельзя (нет параметров ISP, геометрия отвергнута, импорт
    /// не удался, кадр серии с брекетингом). Для машины это жизнь потока, но не готовая картинка.
    Unpresentable,
    /// Поток закрыт: следующий кадр будет другим потоком.
    Inactive,
    /// Новый кадр потока обработан (параметры ISP и геометрия есть): его можно записывать в ISP.
    Processed(ProcessedFrame),
}

/// Факты обработанного нового кадра для решения «показывать ли».
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProcessedFrame {
    /// Поток кадра.
    pub stream: AwbStream,
    /// Известен ли цвет: AWB не скрывает кадр (холодный старт).
    pub color_known: bool,
    /// Готова ли яркость.
    pub exposure: ExposureReadiness,
}

/// Готовность яркости нового потока (решает `ExposureRunner`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExposureReadiness {
    /// Яркость правдоподобна (или ждать нечего).
    Ready,
    /// Цели нет: собственного замера у потока ещё нет и движок не объявил ожидание для камеры кадра.
    WaitingForTarget,
    /// Цель есть (замер потока или объявление движка), но показанный уровень кадра дальше допуска от неё.
    WaitingLevelOff,
}

/// Что положить на экран.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// Живое изображение ISP.
    Live,
    /// Удержанный последний кадр.
    Held,
    /// Чёрный (нечего показывать).
    Black,
}

/// Решение для одного кадра показа (до записи команд).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FramePlan {
    /// Что положить на экран.
    pub screen: Screen,
    /// Начать удержание: отцепить картинку у ISP ДО записи кадра, пока её не перезаписал новый поток.
    pub begins_hold: Option<PictureMeta>,
    /// Закончить удержание, если экран действительно получит живое изображение (или чёрный).
    pub ends_hold: Option<HoldEnd>,
    /// Время показа первого удержанного кадра (для `BackdropContinuity::HeldLastFrame`), если кадр удержанный.
    pub held_since: Option<MonotonicNanos>,
    /// Увеличение удержанной картинки вокруг центра (≥ 1; 1 — без увеличения).
    pub magnification: f32,
}

impl FramePlan {
    /// Что UI узнаёт о фоне под собой (`UiFrameRequest::backdrop`).
    pub fn backdrop(&self) -> BackdropContinuity {
        match (self.screen, self.held_since) {
            (Screen::Held, Some(since)) => BackdropContinuity::HeldLastFrame { since },
            (Screen::Black, _) => BackdropContinuity::NoCameraFrame,
            (Screen::Live, _) | (Screen::Held, None) => BackdropContinuity::Live,
        }
    }
}

/// Что в итоге записано на экран.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Composed {
    /// Живое изображение ISP.
    Live,
    /// Удержанный кадр.
    Held,
    /// Чёрный.
    Black,
}

/// Итог записи кадра для машины.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameOutcome {
    /// Что реально попало на экран.
    pub composed: Composed,
    /// Метаданные нового кадра, записанного в ISP в этом кадре показа (`None` — новый кадр не записан).
    pub new_picture: Option<PictureMeta>,
}

/// Событие жизни удержания (для счётчиков).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldEvent {
    /// Удержание началось.
    Started(HoldReason),
    /// Удержание закончилось; `duration_ms` — от первого удержанного кадра до первого не удержанного.
    Ended { end: HoldEnd, duration_ms: u64 },
}

/// Итог `commit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitReport {
    /// Метка кадра по факту.
    pub label: FrameLabel,
    /// Событие удержания (если было).
    pub event: Option<HoldEvent>,
}

/// Идущее удержание.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Hold {
    /// Время показа первого удержанного кадра.
    since: MonotonicNanos,
    /// Когда в последний раз пришёл любой кадр нового потока, обработанный или нет (или начало удержания).
    last_activity: MonotonicNanos,
    reason: HoldReason,
    /// Удерживаемая картинка.
    held: PictureMeta,
    /// Когда в последний раз пришёл ОБРАБОТАННЫЙ кадр нового потока (или начало удержания).
    last_processed: MonotonicNanos,
    /// Пришёл ли необработанный кадр нового потока после последнего обработанного (поток жив, но
    /// показать нечего).
    unpresentable_since_processed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Live,
    Holding(Hold),
}

/// Машина непрерывности (см. модуль).
#[derive(Debug, Clone)]
pub struct ContinuityMachine {
    config: ContinuityConfig,
    phase: Phase,
    /// Картинка, лежащая в живом изображении ISP и показанная на экране (`None` — такой нет).
    shown: Option<PictureMeta>,
    /// Удержание кончилось честным чёрным (потока не было слишком долго или показать было нечего):
    /// чёрные кадры до первой новой картинки несут эту причину, а не «ещё не было картинки».
    black_after_hold: Option<BlackKind>,
}

impl ContinuityMachine {
    /// Новая машина: картинки ещё не было.
    pub fn new(config: ContinuityConfig) -> Self {
        Self {
            config,
            phase: Phase::Live,
            shown: None,
            black_after_hold: None,
        }
    }

    /// Идёт ли удержание.
    pub fn is_holding(&self) -> bool {
        matches!(self.phase, Phase::Holding(_))
    }

    /// Показанная живая картинка (`None` — нет). Для тестов: боевой код читает решения из `plan`.
    #[cfg(test)]
    pub fn shown_picture(&self) -> Option<PictureMeta> {
        self.shown
    }

    /// Решение для кадра, который будет показан в `now`. `wanted_zoom` — зум, который хочет экран сейчас:
    /// коэффициент и направление шкалы (для увеличения удержанной картинки). Состояние не меняет.
    pub fn plan(&self, now: MonotonicNanos, event: StreamEvent, wanted_zoom: Option<WantedZoom>) -> FramePlan {
        match self.phase {
            Phase::Live => self.plan_live(now, event, wanted_zoom),
            Phase::Holding(hold) => self.plan_holding(&hold, now, event, wanted_zoom),
        }
    }

    fn plan_live(&self, now: MonotonicNanos, event: StreamEvent, wanted_zoom: Option<WantedZoom>) -> FramePlan {
        let Some(shown) = self.shown else {
            // Картинки камеры ещё не было: показываем первый же готовый кадр, иначе чёрный (холодный старт).
            let screen = match event {
                StreamEvent::Processed(frame) if frame.color_known => Screen::Live,
                _ => Screen::Black,
            };
            return FramePlan::plain(screen);
        };
        match event {
            StreamEvent::Quiet | StreamEvent::Unpresentable => FramePlan::plain(Screen::Live),
            StreamEvent::Inactive => self.begin_hold(shown, now, wanted_zoom),
            StreamEvent::Processed(frame) => {
                let ready = frame.color_known && frame.exposure == ExposureReadiness::Ready;
                let same_stream = frame.stream == shown.stream;
                // Тот же поток с известным цветом — обычный живой кадр; другой поток, готовый сразу, —
                // прямая смена без удержания (удерживать нечего: новый кадр можно показать в этот же тик).
                if (same_stream && frame.color_known) || ready {
                    FramePlan::plain(Screen::Live)
                } else {
                    self.begin_hold(shown, now, wanted_zoom)
                }
            }
        }
    }

    fn begin_hold(&self, shown: PictureMeta, now: MonotonicNanos, wanted_zoom: Option<WantedZoom>) -> FramePlan {
        FramePlan {
            screen: Screen::Held,
            begins_hold: Some(shown),
            ends_hold: None,
            held_since: Some(now),
            magnification: held_magnification(&shown, wanted_zoom),
        }
    }

    fn plan_holding(
        &self,
        hold: &Hold,
        now: MonotonicNanos,
        event: StreamEvent,
        wanted_zoom: Option<WantedZoom>,
    ) -> FramePlan {
        let held_plan = || FramePlan {
            screen: Screen::Held,
            begins_hold: None,
            ends_hold: None,
            held_since: Some(hold.since),
            magnification: held_magnification(&hold.held, wanted_zoom),
        };
        match event {
            StreamEvent::Processed(frame) => {
                if frame.color_known && frame.exposure == ExposureReadiness::Ready {
                    FramePlan::revealing(HoldEnd::Condition)
                } else if elapsed_ms(hold.since, now) >= u64::from(self.config.max_hold_ms) {
                    FramePlan::revealing(HoldEnd::Timeout)
                } else {
                    held_plan()
                }
            }
            StreamEvent::Unpresentable => {
                // Кадр пришёл: поток жив, «потока нет» не проверяем. Но показать нечего слишком долго.
                if self.presentable_frame_is_overdue(hold, now) {
                    FramePlan::honest_black(HoldEnd::NoPresentableFrame)
                } else {
                    held_plan()
                }
            }
            StreamEvent::Quiet | StreamEvent::Inactive => {
                if elapsed_ms(hold.last_activity, now) >= u64::from(self.config.max_hold_without_stream_ms) {
                    FramePlan::honest_black(HoldEnd::NoStream)
                } else if hold.unpresentable_since_processed && self.presentable_frame_is_overdue(hold, now) {
                    FramePlan::honest_black(HoldEnd::NoPresentableFrame)
                } else {
                    held_plan()
                }
            }
        }
    }

    /// Поток шлёт кадры, но удержание уже длиннее `max_hold_ms` и обработанного кадра нет столько же:
    /// показывать нечего (например, постоянный отказ параметров ISP). Первое условие — от начала удержания,
    /// второе — от последнего обработанного кадра: редкий сбой среди нормальных кадров удержание не рвёт.
    fn presentable_frame_is_overdue(&self, hold: &Hold, now: MonotonicNanos) -> bool {
        let limit = u64::from(self.config.max_hold_ms);
        elapsed_ms(hold.since, now) >= limit && elapsed_ms(hold.last_processed, now) >= limit
    }

    /// Меняет состояние по тому, что РЕАЛЬНО записано в кадре `plan`.
    pub fn commit(
        &mut self,
        now: MonotonicNanos,
        event: StreamEvent,
        plan: &FramePlan,
        outcome: &FrameOutcome,
    ) -> CommitReport {
        let mut hold_event = None;
        // 1. Начало удержания: живая картинка отцеплена (её держит владелец GPU), живой больше нет.
        if let Some(held) = plan.begins_hold {
            let reason = hold_reason(event);
            self.phase = Phase::Holding(Hold {
                since: now,
                last_activity: now,
                reason,
                held,
                last_processed: now,
                unpresentable_since_processed: false,
            });
            self.shown = None;
            hold_event = Some(HoldEvent::Started(reason));
        }
        // 2. Активность нового потока: любой пришедший кадр — жизнь потока; обработанный ещё и сбрасывает
        // отсчёт «показать нечего».
        if let Phase::Holding(hold) = &mut self.phase {
            match event {
                StreamEvent::Processed(_) => {
                    hold.last_activity = now;
                    hold.last_processed = now;
                    hold.unpresentable_since_processed = false;
                }
                StreamEvent::Unpresentable => {
                    hold.last_activity = now;
                    hold.unpresentable_since_processed = true;
                }
                StreamEvent::Quiet | StreamEvent::Inactive => {}
            }
        }
        // 3. Конец удержания — только если экран получил то, что задумано (живое или честный чёрный).
        if let (Some(end), Phase::Holding(hold)) = (plan.ends_hold, self.phase) {
            let ended = match (end, outcome.composed) {
                (HoldEnd::Condition | HoldEnd::Timeout, Composed::Live) => true,
                (HoldEnd::NoStream | HoldEnd::NoPresentableFrame, Composed::Black) => true,
                // Запись не удалась (например, ISP отказал и экран получил удержанный): удержание идёт дальше.
                _ => false,
            };
            if ended {
                self.phase = Phase::Live;
                hold_event = Some(HoldEvent::Ended {
                    end,
                    duration_ms: elapsed_ms(hold.since, now),
                });
                match end {
                    HoldEnd::NoStream => self.black_after_hold = Some(BlackKind::CameraUnavailable),
                    HoldEnd::NoPresentableFrame => self.black_after_hold = Some(BlackKind::PictureUnavailable),
                    HoldEnd::Condition | HoldEnd::Timeout => {}
                }
                if self.black_after_hold.is_some() {
                    self.shown = None;
                }
            }
        }
        // 4. Живая картинка: новый кадр, лёгший на экран живым.
        if outcome.composed == Composed::Live
            && let Some(picture) = outcome.new_picture
        {
            self.shown = Some(picture);
            self.black_after_hold = None;
        }
        CommitReport {
            label: self.label_of(outcome.composed),
            event: hold_event,
        }
    }

    /// Метка кадра по факту записи. Чёрный различается по причине.
    fn label_of(&self, composed: Composed) -> FrameLabel {
        match composed {
            Composed::Live => FrameLabel::Live,
            Composed::Held => FrameLabel::Transition(TransitionKind::HeldLastFrame),
            Composed::Black if let Some(kind) = self.black_after_hold => FrameLabel::Black(kind),
            Composed::Black => FrameLabel::Black(match self.phase {
                Phase::Holding(_) => BlackKind::PictureExpected,
                Phase::Live if self.shown.is_some() => BlackKind::PictureExpected,
                Phase::Live => BlackKind::NoPicture,
            }),
        }
    }
}

impl FramePlan {
    /// Тот же план, когда живого изображения на самом деле нет (его не показывали или сабмит не принят):
    /// живой экран невозможен — чёрный. Удержанное и честный чёрный остаются как есть.
    pub fn without_live_image(self) -> Self {
        match self.screen {
            Screen::Live if self.ends_hold.is_none() => Self {
                screen: Screen::Black,
                ..self
            },
            _ => self,
        }
    }

    /// Обычный кадр без удержания.
    fn plain(screen: Screen) -> Self {
        Self {
            screen,
            begins_hold: None,
            ends_hold: None,
            held_since: None,
            magnification: 1.0,
        }
    }

    /// Честный чёрный, которым заканчивается удержание (потока нет или показывать нечего).
    fn honest_black(end: HoldEnd) -> Self {
        Self {
            ends_hold: Some(end),
            ..Self::plain(Screen::Black)
        }
    }

    /// Живой кадр, которым заканчивается удержание.
    fn revealing(end: HoldEnd) -> Self {
        Self {
            ends_hold: Some(end),
            ..Self::plain(Screen::Live)
        }
    }
}

/// Сколько мс прошло от `earlier` до `later` (0, если часы пошли назад).
fn elapsed_ms(earlier: MonotonicNanos, later: MonotonicNanos) -> u64 {
    let nanos = later.as_nanos().saturating_sub(earlier.as_nanos());
    u64::try_from(nanos / NANOS_PER_MILLI).unwrap_or(0)
}

/// Увеличение удержанной картинки вокруг центра (решение 112): пока палец крутит колесо во время смены
/// объектива, удержанный кадр масштабируется за ним. `wanted` — коэффициент и направление шкалы, которые
/// хочет экран; картинка показывает `held.shown_ratio` на шкале СВОЕГО направления. Увеличиваем только
/// когда направление картинки известно и совпадает с направлением шкалы экрана: при перевороте на другую
/// сторону (или неизвестном направлении картинки) шкалы несравнимы — увеличения нет. Не меньше 1:
/// уменьшение дало бы чёрные поля.
pub fn held_magnification(held: &PictureMeta, wanted: Option<WantedZoom>) -> f32 {
    let Some(wanted) = wanted else {
        return 1.0;
    };
    if held.facing != Some(wanted.facing) {
        return 1.0;
    }
    let ratio = wanted.ratio.value() / held.shown_ratio.value();
    if ratio.is_finite() { ratio.max(1.0) } else { 1.0 }
}

/// Почему начинается удержание: какое из условий показа нового кадра не выполнено (см. `plan_live`).
fn hold_reason(event: StreamEvent) -> HoldReason {
    match event {
        StreamEvent::Processed(frame) if !frame.color_known => HoldReason::ColorUnknown,
        StreamEvent::Processed(frame) => match frame.exposure {
            ExposureReadiness::WaitingLevelOff => HoldReason::ExposureLevelOff,
            // `Ready` с известным цветом удержание не начинает; если сюда всё же попали — честнее «нет цели»,
            // чем молча назвать причиной уровень.
            ExposureReadiness::WaitingForTarget | ExposureReadiness::Ready => HoldReason::ExposureWithoutTarget,
        },
        // Удержание начинается только на кадре или пропаже потока; остальное — пропажа потока.
        StreamEvent::Inactive | StreamEvent::Quiet | StreamEvent::Unpresentable => HoldReason::StreamInactive,
    }
}

#[cfg(test)]
#[path = "continuity_tests.rs"]
mod tests;
