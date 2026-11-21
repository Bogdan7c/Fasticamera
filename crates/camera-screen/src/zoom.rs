//! Состояние зума экрана: **единственный владелец** значения, которое видит пользователь (решение 93:
//! у значения один владелец, как у EV). Лента кнопок, колесо и щипок значение только показывают и
//! предлагают; хозяин — [`ZoomState`], меняют его редьюсер ([`crate::reduce`]) и только он.
//!
//! # Что внутри
//! * направление (`facing`) и коэффициент (`ratio`) — то, что экран показывает и публикует движку намерением;
//! * «усыновление»: на старте у экрана нет активной камеры, и ставить 1× «от балды» нельзя — переустановленный
//!   экран сбросил бы объектив пользователя. Поэтому коэффициент **усыновляется** из первых фактов с активной
//!   камерой (без намерения наружу, [`ZoomState::adopt`]);
//! * плавный переход к упору по тапу на кнопке ([`Glide`]): кривая — smoothstep по логарифму, длительность из
//!   конфига `[measured: R00 M:124]`;
//! * колесо: открыто ли, лежит ли на нём палец, когда была последняя активность (для ухода через ≈ 2,15 с);
//! * щипок: коэффициент в начале жеста;
//! * спрос на тёплого соседа (13b, решения 117 и 124): нажатие на ленту, ставшее дольше тапа, открытое колесо и
//!   щипок значат «соседняя камера скоро понадобится». Это **выведенный факт** — им владеет `ZoomState`, а
//!   редьюсер публикует каждый его переход движку (экран о камерах ничего не знает, соседей выбирает движок);
//! * метка переворота: номер и момент (по ней UI рисует карточку);
//! * наблюдатель неудач смены объектива ([`FailureWatch`]): учтённый номер не откатывает повторно.
//!
//! Инварианты: коэффициент всегда лежит в пределах группы текущего направления (проверяет редьюсер, он же
//! держит группу); последний «задний» коэффициент помнится для возврата с фронтальной камеры.

use camera_contract::ids::LensFacing;
use camera_contract::zoom::{LensGroup, StandbyDemand, ZoomRatio};
use shell_contract::clock::MonotonicNanos;

/// Допуск сравнения коэффициента с упором (относительный): упор «1×» не должен промахиваться на ошибку f32.
const STOP_MATCH_TOLERANCE: f32 = 1.0e-3;

/// Усыновлено ли значение из фактов камеры.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Adoption {
    /// Активной камеры ещё не было: значение условное (1×, задняя), намерение по нему не публиковалось.
    Pending,
    /// Значение взято у камеры или выбрано пользователем.
    Adopted,
}

/// Следит за номерами неудач смены объектива.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureWatch {
    /// Фактов ещё не было: что считать «уже учтённым», неизвестно.
    Unarmed,
    /// Все неудачи с номером ≤ `last_seen` учтены. Первые факты после создания экрана считаются учтёнными
    /// целиком: старая неудача не должна откатывать только что созданный экран.
    Armed { last_seen: Option<u64> },
}

/// Переход к упору по тапу на кнопке.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Glide {
    from: ZoomRatio,
    to: ZoomRatio,
    started_at: MonotonicNanos,
    duration_nanos: i64,
}

/// Шаг перехода на момент `now`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GlideStep {
    pub ratio: ZoomRatio,
    /// Переход дошёл до цели и снят.
    pub finished: bool,
}

/// Колесо: открыто ли и чем занято.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum WheelPhase {
    Closed,
    Open {
        /// Последнее движение: касание, сдвиг или отпускание.
        last_activity: MonotonicNanos,
        finger_down: bool,
    },
}

/// Палец лежит на ленте кнопок зума. Тап и долгое нажатие неотличимы, пока палец не пролежал дольше порога
/// тапа (`zoom.standby.press_threshold_ms`): до порога тёплую камеру будить нельзя (решение 117: тап по `.6`/`1`
/// её не будит), после — это уже «зум, а не тап» (решение 124).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct StripPress {
    /// Когда палец лёг (из времени действия, не из часов).
    since: MonotonicNanos,
    /// Палец пролежал дольше порога тапа; признак защёлкивается и не откатывается до отпускания.
    outlasted_tap: bool,
}

/// Метка переворота камеры: растёт номером, чтобы UI отличал новый переворот от уже нарисованного.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlipMark {
    /// Сколько переворотов было за жизнь экрана.
    pub sequence: u32,
    /// Когда переворот запрошен.
    pub at: MonotonicNanos,
    /// Куда смотрит камера после переворота.
    pub to: LensFacing,
}

/// Состояние зума.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoomState {
    adoption: Adoption,
    facing: LensFacing,
    ratio: ZoomRatio,
    /// Последний коэффициент задней камеры: возврат с фронталки восстанавливает его (решение 115).
    last_back_ratio: ZoomRatio,
    glide: Option<Glide>,
    wheel: WheelPhase,
    /// Коэффициент в начале идущего щипка; `Some` только пока щипок идёт.
    pinch_start: Option<ZoomRatio>,
    /// Палец на ленте кнопок; `Some` только пока он лежит.
    strip_press: Option<StripPress>,
    /// Спрос на тёплого соседа, который экран **уже опубликовал** движку последним намерением. Сравнение с
    /// выведенным значением ([`Self::derived_standby_demand`]) и есть «переход», который надо отправить.
    published_standby: StandbyDemand,
    /// Сколько раз спрос менялся за жизнь экрана (диагностика).
    standby_transitions: u64,
    flip: Option<FlipMark>,
    pub(crate) failure_watch: FailureWatch,
}

impl ZoomState {
    pub(crate) fn new() -> Self {
        Self {
            adoption: Adoption::Pending,
            facing: LensFacing::Back,
            ratio: ZoomRatio::ONE,
            last_back_ratio: ZoomRatio::ONE,
            glide: None,
            wheel: WheelPhase::Closed,
            pinch_start: None,
            strip_press: None,
            published_standby: StandbyDemand::NotNeeded,
            standby_transitions: 0,
            flip: None,
            failure_watch: FailureWatch::Unarmed,
        }
    }

    // ---- чтение ----

    /// Направление, которое показывает экран.
    pub fn facing(&self) -> LensFacing {
        self.facing
    }

    /// Коэффициент, который показывает экран (во время перехода — текущий промежуточный).
    pub fn ratio(&self) -> ZoomRatio {
        self.ratio
    }

    /// Взято ли значение у камеры (или выбрано пользователем). До этого экран показывает условные 1×.
    pub fn is_adopted(&self) -> bool {
        self.adoption == Adoption::Adopted
    }

    /// Последний коэффициент задней камеры.
    pub fn last_back_ratio(&self) -> ZoomRatio {
        self.last_back_ratio
    }

    /// Открыто ли колесо (лента кнопок в это время скрыта).
    pub fn is_wheel_open(&self) -> bool {
        matches!(self.wheel, WheelPhase::Open { .. })
    }

    /// Лежит ли палец на колесе.
    pub fn is_wheel_touched(&self) -> bool {
        matches!(self.wheel, WheelPhase::Open { finger_down: true, .. })
    }

    /// Идёт ли щипок.
    pub fn is_pinching(&self) -> bool {
        self.pinch_start.is_some()
    }

    /// Спрос на тёплого соседа, который диктует **текущее** состояние жестов (решения 117 и 124): нажатие на
    /// ленте дольше тапа, открытое колесо или щипок. У фронтальной камеры соседей нет — спроса нет, чтобы не
    /// слать движку пустых просьб. До первого значения от пользователя или камеры спроса нет тоже.
    pub fn derived_standby_demand(&self) -> StandbyDemand {
        let press_outlasted_tap = self.strip_press.is_some_and(|press| press.outlasted_tap);
        let wanted = self.facing == LensFacing::Back
            && self.is_adopted()
            && (press_outlasted_tap || self.is_wheel_open() || self.is_pinching());
        if wanted {
            StandbyDemand::KeepNeighbourWarm
        } else {
            StandbyDemand::NotNeeded
        }
    }

    /// Спрос, уже опубликованный движку (последнее намерение).
    pub fn published_standby_demand(&self) -> StandbyDemand {
        self.published_standby
    }

    /// Сколько раз спрос на тёплого соседа менялся (диагностика).
    pub fn standby_transitions(&self) -> u64 {
        self.standby_transitions
    }

    /// Идёт ли плавный переход к упору.
    pub fn is_gliding(&self) -> bool {
        self.glide.is_some()
    }

    /// Последний переворот камеры (`None` — переворотов не было).
    pub fn last_flip(&self) -> Option<FlipMark> {
        self.flip
    }

    /// Номер выбранной кнопки ленты: упор, к которому идёт переход, а без перехода — наибольший упор не выше
    /// текущего значения (на 1,7× выбрана кнопка «1», на 2× — «2»). Пока значение ниже первого упора — первая.
    pub fn selected_stop_index(&self, group: &LensGroup) -> usize {
        let reference = self.glide.map_or(self.ratio, |glide| glide.to);
        let value = reference.value();
        group
            .stops
            .iter()
            .rposition(|stop| stop.ratio.value() <= value * (1.0 + STOP_MATCH_TOLERANCE))
            .unwrap_or(0)
    }

    // ---- изменение (только редьюсер) ----

    /// Усыновить значение из фактов камеры: без намерения наружу.
    pub(crate) fn adopt(&mut self, facing: LensFacing, ratio: ZoomRatio) {
        self.adoption = Adoption::Adopted;
        self.facing = facing;
        self.glide = None;
        self.set_ratio(ratio);
    }

    /// Поставить коэффициент (уже зажатый редьюсером в пределы группы). Запоминает «задний» для возврата.
    pub(crate) fn set_ratio(&mut self, ratio: ZoomRatio) {
        self.ratio = ratio;
        if self.facing == LensFacing::Back {
            self.last_back_ratio = ratio;
        }
    }

    /// Пользователь взял значение сам: условное значение стало настоящим.
    pub(crate) fn mark_user_chosen(&mut self) {
        self.adoption = Adoption::Adopted;
    }

    pub(crate) fn start_glide(&mut self, to: ZoomRatio, now: MonotonicNanos, duration_ms: f64) {
        self.mark_user_chosen();
        self.glide = Some(Glide {
            from: self.ratio,
            to,
            started_at: now,
            duration_nanos: (duration_ms * 1_000_000.0).round() as i64,
        });
    }

    pub(crate) fn cancel_glide(&mut self) {
        self.glide = None;
    }

    /// Продвинуть переход на `now`. `None` — перехода нет.
    pub(crate) fn glide_step(&self, now: MonotonicNanos) -> Option<GlideStep> {
        let glide = self.glide?;
        let elapsed = now.as_nanos().saturating_sub(glide.started_at.as_nanos());
        if elapsed >= glide.duration_nanos {
            return Some(GlideStep {
                ratio: glide.to,
                finished: true,
            });
        }
        let progress = (elapsed as f64 / glide.duration_nanos as f64).clamp(0.0, 1.0);
        // smoothstep: кривая прогресса ленты `[measured: R00 M:124]` (±0,02 на таблице кадров).
        let eased = progress * progress * (3.0 - 2.0 * progress);
        // Равномерно по логарифму: октаву зум проходит с одной «скоростью», как колесо.
        let ratio =
            f64::from(glide.from.value()) * (f64::from(glide.to.value()) / f64::from(glide.from.value())).powf(eased);
        Some(GlideStep {
            ratio: ZoomRatio::new(ratio as f32).unwrap_or(glide.to),
            finished: false,
        })
    }

    pub(crate) fn open_wheel(&mut self, at: MonotonicNanos) {
        if !self.is_wheel_open() {
            self.wheel = WheelPhase::Open {
                last_activity: at,
                finger_down: false,
            };
        }
    }

    pub(crate) fn wheel_finger(&mut self, down: bool, at: MonotonicNanos) {
        if let WheelPhase::Open { .. } = self.wheel {
            self.wheel = WheelPhase::Open {
                last_activity: at,
                finger_down: down,
            };
        }
    }

    /// Любое движение значения на колесе продлевает его жизнь.
    pub(crate) fn note_wheel_activity(&mut self, at: MonotonicNanos) {
        if let WheelPhase::Open { finger_down, .. } = self.wheel {
            self.wheel = WheelPhase::Open {
                last_activity: at,
                finger_down,
            };
        }
    }

    /// Закрыть колесо, если палец не лежит и с последней активности прошло не меньше `idle_nanos`.
    /// Возвращает `true`, если колесо закрылось.
    pub(crate) fn close_wheel_if_idle(&mut self, now: MonotonicNanos, idle_nanos: i64) -> bool {
        match self.wheel {
            WheelPhase::Open {
                last_activity,
                finger_down: false,
            } if now.as_nanos().saturating_sub(last_activity.as_nanos()) >= idle_nanos => {
                self.wheel = WheelPhase::Closed;
                true
            }
            _ => false,
        }
    }

    /// Подогнать направление под реально активную камеру (синхронизация по фактам, без намерения).
    pub(crate) fn sync_facing(&mut self, facing: LensFacing) {
        self.facing = facing;
    }

    /// Закрыть колесо сразу (панель открылась поверх: зум скрывается вместе с нижним рядом).
    pub(crate) fn close_wheel(&mut self) {
        self.wheel = WheelPhase::Closed;
    }

    /// Палец лёг на ленту кнопок в момент `at`. Повторное «лёг» без отпускания не сдвигает начало.
    pub(crate) fn begin_strip_press(&mut self, at: MonotonicNanos) {
        if self.strip_press.is_none() {
            self.strip_press = Some(StripPress {
                since: at,
                outlasted_tap: false,
            });
        }
    }

    /// Палец поднят с ленты (или жест отменён). Возвращает `true`, если нажатие было.
    pub(crate) fn end_strip_press(&mut self) -> bool {
        self.strip_press.take().is_some()
    }

    /// Защёлкнуть «дольше тапа», если палец лежит не меньше `threshold_nanos` на момент `now`.
    pub(crate) fn promote_strip_press(&mut self, now: MonotonicNanos, threshold_nanos: i64) {
        if let Some(press) = &mut self.strip_press
            && !press.outlasted_tap
            && now.as_nanos().saturating_sub(press.since.as_nanos()) >= threshold_nanos
        {
            press.outlasted_tap = true;
        }
    }

    /// Записать спрос, ушедший движку в намерении. Меняет счётчик переходов только при реальной смене.
    pub(crate) fn record_published_standby(&mut self, demand: StandbyDemand) {
        if self.published_standby != demand {
            self.published_standby = demand;
            self.standby_transitions = self.standby_transitions.saturating_add(1);
        }
    }

    pub(crate) fn begin_pinch(&mut self) {
        self.mark_user_chosen();
        self.pinch_start = Some(self.ratio);
    }

    pub(crate) fn pinch_start_ratio(&self) -> Option<ZoomRatio> {
        self.pinch_start
    }

    pub(crate) fn end_pinch(&mut self) {
        self.pinch_start = None;
    }

    /// Перевернуть камеру: направление, коэффициент и метка. Колесо, переход и щипок гаснут.
    pub(crate) fn flip_to(&mut self, facing: LensFacing, ratio: ZoomRatio, at: MonotonicNanos) {
        self.mark_user_chosen();
        self.facing = facing;
        self.glide = None;
        self.pinch_start = None;
        self.strip_press = None;
        self.wheel = WheelPhase::Closed;
        self.set_ratio(ratio);
        let sequence = self.flip.map_or(1, |mark| mark.sequence.saturating_add(1));
        self.flip = Some(FlipMark {
            sequence,
            at,
            to: facing,
        });
    }
}

impl Default for ZoomState {
    fn default() -> Self {
        Self::new()
    }
}
