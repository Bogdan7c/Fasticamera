//! Картинка миниатюры и её растворение (сессия 14g): всё, что экран знает о последнем сохранённом снимке.
//!
//! # Кто чем владеет
//! * [`ThumbnailTracker`] — единственный владелец: «входящая» картинка (до применения итога), растворение
//!   старой картинки в новую, времена «нажатие → показ» и учёт нажатий. Лежит в `ScreenState`, меняется только
//!   редьюсером (`CaptureFinished`, `Tick`) и двумя методами фасада экрана (положить/убрать входящую картинку).
//! * Какой снимок «последний» — по-прежнему [`crate::shutter::ThumbnailState`] (правило решения 154: тот,
//!   чьё «сохранено» пришло ПОСЛЕДНИМ, а не последним нажатый).
//!
//! # Как картинка попадает в состояние
//! `ScreenAction` пикселей не несёт (чистый редьюсер, дешёвое сравнение). Приложение отдаёт картинку фасаду
//! ([`crate::CameraScreen::handle_saved_with_picture`]): тот кладёт её во «входящий» ящик трекера, прогоняет
//! обычное `CaptureFinished(Saved)` и ящик очищает. Редьюсер забирает картинку из ящика ТОЛЬКО для снимка, чей
//! итог принимает; итог, отвергнутый как устаревший, картинку не ставит (ящик чистится с учётом).
//!
//! # Растворение
//! Кривая — линейная, длительность — `capture.thumbnail_fade_ms` (150 мс `[measured]` 0,13–0,17 с, строка
//! «Миниатюра после снимка» в `docs/ios-camera/motion.md`; линейность `[estimate]`: замером кривая не снята).
//! Старая картинка рисуется ПОД новой полностью непрозрачной, новая сверху с прозрачностью `t`: настоящее
//! перекрёстное растворение, сквозь которое стекло не просвечивает.
//!
//! Начало растворения «защёлкивается» на первом `Tick`, пришедшем ПОСЛЕ появления картинки, то есть на
//! первом кадре, который её рисует. Время прихода итога для этого не годится: после сворачивания приложения
//! итог лежит в очереди минуты, и растворение прошло бы невидимо. Отсюда же — известное ограничение: если
//! сам кадр не удалось собрать (ошибка вида после `Tick`), растворение уже пошло.
//!
//! # Известное ограничение (14g)
//! Миниатюра живёт в экране. Пока экран стоит установленным (сворачивание, потеря поверхности, 14e), очередь
//! его входа ждёт, а при возврате `apply_pending_input` применяет итог и картинку. Но переустановка экрана
//! (сценарий снимает экран на время прогона, `DroppedByRenderer`) создаёт НОВЫЙ экран с пустой миниатюрой до
//! следующего снимка (решение 156: превью из галереи при запуске — бэклог); механизма переноса картинки
//! в новый экран нет.
//!
//! Новое «сохранено» посреди растворения: «старой» становится картинка, которая сейчас проявлялась
//! (скачок к ней — осознанная цена, чтобы никогда не рисовать больше двух картинок), растворение начинается
//! заново. Если текущая картинка ещё НИ РАЗУ не рисовалась (итоги пришли, пока экрана на поверхности не было),
//! «старой» остаётся та, что пользователь видел раньше: невидимую картинку не растворяем.

use std::collections::VecDeque;

use shell_contract::clock::MonotonicNanos;
use ui_draw::picture::UiPicture;

use crate::shutter::{ShotRequestId, ThumbnailState};

/// Наносекунд в миллисекунде.
const NANOS_PER_MS: f64 = 1_000_000.0;

/// Растворение: где мы сейчас.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum FadePhase {
    /// Растворения нет: рисуется только текущая картинка, полностью.
    #[default]
    Settled,
    /// Новая картинка пришла, но кадра с ней ещё не было: старт будет на первом `Tick`.
    WaitingForFirstFrame,
    /// Идёт: с этого момента (время `Tick` первого кадра).
    Running { started_at: MonotonicNanos },
}

/// Что рисовать в круге миниатюры в этот момент.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThumbnailBlend<'a> {
    /// Картинка под новой, рисуется полностью непрозрачной; `None` — под новой только стекло.
    pub underlay: Option<&'a UiPicture>,
    /// Прозрачность новой картинки: доля растворения от 0 до 1.
    pub overlay_opacity: f32,
}

/// Времена одной показанной картинки: нужны замеру «нажатие → картинка».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PictureTiming {
    /// Чей снимок.
    pub shot: ShotRequestId,
    /// Момент нажатия (`ShotRequest::pressed_at`): для таймера — момент срабатывания отсчёта.
    pub pressed_at: MonotonicNanos,
    /// Время показа кадра, на котором картинка впервые нарисована; `None` — ещё не рисовалась.
    pub shown_at: Option<MonotonicNanos>,
}

impl PictureTiming {
    /// «Нажатие → картинка», мс; `None`, пока картинка не показана.
    pub fn press_to_picture_ms(&self) -> Option<f64> {
        let shown_at = self.shown_at?;
        Some((shown_at.as_nanos().saturating_sub(self.pressed_at.as_nanos())) as f64 / NANOS_PER_MS)
    }
}

/// Счётчики миниатюры (ничего не пропадает молча).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ThumbnailCounters {
    /// «Сохранено» пришло без картинки: в круге серый диск.
    pub saved_without_picture: u64,
    /// Картинки, нарисованные впервые (по одной на показанный снимок).
    pub pictures_shown: u64,
    /// Картинки, приехавшие с итогом, который экран отверг как устаревший.
    pub pictures_rejected_with_stale_result: u64,
    /// Моменты нажатия, вытесненные из ограниченного журнала до итога снимка.
    pub press_times_forgotten: u64,
}

/// Владелец картинки миниатюры, растворения и замера (см. доку модуля).
#[derive(Debug, Clone, PartialEq)]
pub struct ThumbnailTracker {
    /// Длительность растворения, мс (из конфига на момент создания экрана).
    fade_ms: u32,
    /// Ёмкость журнала моментов нажатия.
    press_capacity: usize,
    /// Картинка, которую фасад положил к итогу, пока тот не применён.
    incoming: Option<(ShotRequestId, UiPicture)>,
    /// Нажатия, чьего итога ещё не было: по порядку нажатий.
    pressed: VecDeque<(ShotRequestId, MonotonicNanos)>,
    /// Картинка, из которой растворяется текущая.
    previous: Option<UiPicture>,
    phase: FadePhase,
    timing: Option<PictureTiming>,
    counters: ThumbnailCounters,
}

impl ThumbnailTracker {
    /// `press_capacity` — сколько нажатий помнить до итога (не меньше 1).
    pub(crate) fn new(fade_ms: u32, press_capacity: usize) -> Self {
        Self {
            fade_ms,
            press_capacity: press_capacity.max(1),
            incoming: None,
            pressed: VecDeque::new(),
            previous: None,
            phase: FadePhase::Settled,
            timing: None,
            counters: ThumbnailCounters::default(),
        }
    }

    /// Запоминает момент нажатия снимка. Журнал ограничен: самое старое вытесняется со счётчиком.
    pub(crate) fn note_press(&mut self, request: ShotRequestId, pressed_at: MonotonicNanos) {
        if self.pressed.len() >= self.press_capacity {
            self.pressed.pop_front();
            self.counters.press_times_forgotten += 1;
        }
        self.pressed.push_back((request, pressed_at));
    }

    /// Снимок закончился (удачно или нет): момент нажатия больше не нужен. Возвращает его, если помнили.
    fn take_press(&mut self, request: ShotRequestId) -> Option<MonotonicNanos> {
        let index = self.pressed.iter().position(|(pressed, _)| *pressed == request)?;
        self.pressed.remove(index).map(|(_, pressed_at)| pressed_at)
    }

    /// Фасад кладёт картинку к итогу `request` (до применения `CaptureFinished`).
    pub(crate) fn stage_incoming(&mut self, request: ShotRequestId, picture: UiPicture) {
        self.incoming = Some((request, picture));
    }

    /// Фасад убирает неиспользованную картинку после применения итога. `true` — картинка осталась лишней
    /// (итог отвергнут), это считается.
    pub(crate) fn discard_incoming(&mut self) -> bool {
        let leftover = self.incoming.take().is_some();
        if leftover {
            self.counters.pictures_rejected_with_stale_result += 1;
        }
        leftover
    }

    /// Снимок сохранён: возвращает новое состояние миниатюры.
    ///
    /// `current` — то, что показано сейчас; с картинкой снимка запускается растворение, без неё в круге
    /// серый диск (счётчик).
    pub(crate) fn on_saved(&mut self, request: ShotRequestId, current: &ThumbnailState) -> ThumbnailState {
        let pressed_at = self.take_press(request);
        let picture = match self.incoming.take() {
            Some((staged_for, picture)) if staged_for == request => Some(picture),
            // Картинка чужого снимка к этому итогу не относится: она не показывается и не теряется молча.
            Some(_) => {
                self.counters.pictures_rejected_with_stale_result += 1;
                None
            }
            None => None,
        };
        let Some(new_picture) = picture else {
            self.counters.saved_without_picture += 1;
            self.previous = None;
            self.phase = FadePhase::Settled;
            self.timing = None;
            return ThumbnailState::Last {
                shot: request,
                picture: None,
            };
        };
        // Невидимую (ещё не нарисованную) картинку не растворяем: «старой» остаётся уже виденная.
        if self.phase != FadePhase::WaitingForFirstFrame {
            self.previous = match current {
                ThumbnailState::Last {
                    picture: Some(shown), ..
                } => Some(shown.clone()),
                ThumbnailState::Last { picture: None, .. } | ThumbnailState::Empty => None,
            };
        }
        self.phase = FadePhase::WaitingForFirstFrame;
        self.timing = pressed_at.map(|pressed_at| PictureTiming {
            shot: request,
            pressed_at,
            shown_at: None,
        });
        ThumbnailState::Last {
            shot: request,
            picture: Some(new_picture),
        }
    }

    /// Снимок не получился: момент нажатия забывается, картинка (старая) остаётся как была.
    pub(crate) fn on_failed(&mut self, request: ShotRequestId) {
        let _forgotten = self.take_press(request);
    }

    /// Время кадра: защёлкивает начало растворения и завершает его, когда время вышло.
    pub(crate) fn on_tick(&mut self, now: MonotonicNanos) {
        match self.phase {
            FadePhase::Settled => {}
            FadePhase::WaitingForFirstFrame => {
                self.phase = FadePhase::Running { started_at: now };
                self.counters.pictures_shown += 1;
                if let Some(timing) = &mut self.timing {
                    timing.shown_at = Some(now);
                }
            }
            FadePhase::Running { started_at } => {
                if self.fraction_at(started_at, now) >= 1.0 {
                    self.previous = None;
                    self.phase = FadePhase::Settled;
                }
            }
        }
    }

    /// Доля растворения от начала `started_at` к моменту `now`: линейно, зажато в 0..1.
    fn fraction_at(&self, started_at: MonotonicNanos, now: MonotonicNanos) -> f32 {
        let duration_nanos = f64::from(self.fade_ms) * NANOS_PER_MS;
        let elapsed_nanos = now.as_nanos().saturating_sub(started_at.as_nanos()) as f64;
        // Конфиг проверен: длительность > 0; `max` — страховка от деления на ноль.
        (elapsed_nanos / duration_nanos.max(1.0)).clamp(0.0, 1.0) as f32
    }

    /// Что рисовать в момент `now`.
    pub fn blend(&self, now: MonotonicNanos) -> ThumbnailBlend<'_> {
        match self.phase {
            FadePhase::Settled => ThumbnailBlend {
                underlay: None,
                overlay_opacity: 1.0,
            },
            FadePhase::WaitingForFirstFrame => ThumbnailBlend {
                underlay: self.previous.as_ref(),
                overlay_opacity: 0.0,
            },
            FadePhase::Running { started_at } => {
                let fraction = self.fraction_at(started_at, now);
                ThumbnailBlend {
                    // Растворение дошло до конца, а `Tick` его ещё не завершил: нижнюю картинку уже не рисуем.
                    underlay: if fraction < 1.0 { self.previous.as_ref() } else { None },
                    overlay_opacity: fraction,
                }
            }
        }
    }

    /// Времена последней картинки с известным моментом нажатия.
    pub fn timing(&self) -> Option<PictureTiming> {
        self.timing
    }

    pub fn counters(&self) -> ThumbnailCounters {
        self.counters
    }
}

#[cfg(test)]
#[path = "thumbnail_tests.rs"]
mod tests;
