//! K3 «Шкала с упорами»: модель значений + движок прокрутки + события.
//!
//! # Владение
//! Шкала владеет моделью, приводом оси и очередью событий. Экран владеет ареной жестов: он
//! передаёт данные жестов намерениями (`catch`, `drag_*`, `release_without_drag`), раз в кадр зовёт
//! [`SnapScale::advance`] и забирает события через [`SnapScale::drain_events`].
//!
//! # Единицы
//! Модель измеряет положение в «единицах», шкала переводит их в pt токеном плотности:
//! `смещение_pt = положение × pt_на_единицу`. Смещение контента растёт вместе со значением.

use ui_core::geometry::Vector;
use ui_core::gesture::PanTranslation;
use ui_core::scroll::ReleaseOutcome;
use ui_core::time::{MonotonicNanos, MotionPreference};

use super::detents::{Detent, build_detents, crossed};
use super::error::ScaleError;
use super::events::{ScaleEvent, ScaleMark, ScaleSnapshot, SetValueOutcome, TravelDirection};
use super::spec::{ScaleSpec, SnapPolicy, TickVisibility};
use super::zero_magnet::ZeroMagnet;
use crate::drive::{AxisDrive, DriveSetup, MotionPhase, OffsetRange, ScrollTransition};
use crate::environment::ElementEnvironment;
use crate::event_queue::{EventOrigin, EventQueue};
use crate::pan::PanUpdate;
use crate::tokens::{HapticEvent, HapticTable, SnapRole};
use crate::value::{Emphasis, Quantization, ValueModel};

/// Всё, что шкала выводит из модели: рубежи, опорные положения и точки прилипания. Считается
/// целиком до применения, чтобы смена пределов могла быть атомарной.
struct MarkLayout {
    detents: Vec<Detent>,
    /// Положения опорных значений, единицы модели (кандидаты в магниты).
    reference_positions: Vec<f64>,
    /// Точки прилипания в pt (пусто у плавной шкалы с магнитами).
    snap_points: Vec<f64>,
}

/// Шкала с упорами над моделью значений `M`. Модель клонируется при смене пределов (чтобы смена
/// была атомарной), поэтому `M: Clone`.
pub struct SnapScale<M: ValueModel + Clone> {
    model: M,
    spec: ScaleSpec,
    /// pt на единицу модели (токен плотности).
    points_per_unit: f64,
    /// Радиус магнита, pt (токен).
    magnet_capture_points: f64,
    haptics: HapticTable,
    drive: AxisDrive,
    detents: Vec<Detent>,
    /// Положения опорных значений, единицы модели (кандидаты в магниты).
    reference_positions: Vec<f64>,
    // --- наблюдение за движением между вызовами `advance` ---
    last_position: f64,
    last_value: M::Value,
    last_phase: MotionPhase,
    /// Чьё движение сейчас: от него зависит, вибрируем ли.
    origin: EventOrigin,
    events: EventQueue<ScaleEvent<M::Value>>,
}

impl<M: ValueModel + Clone> SnapScale<M> {
    /// Создать шкалу в покое на `initial_value`. Значение вне пределов зажимается правилом модели.
    pub fn new(
        model: M,
        initial_value: M::Value,
        spec: ScaleSpec,
        environment: &ElementEnvironment<'_>,
        now: MonotonicNanos,
    ) -> Result<Self, ScaleError> {
        if spec.snap == SnapPolicy::ContinuousWithMagnets && model.quantization() == Quantization::Stepped {
            return Err(ScaleError::MagnetsRequireContinuousModel);
        }
        let points_per_unit = environment.tokens.density.points_per_unit(spec.density);
        let initial_position = model.position_of(initial_value)?;
        let mut drive = AxisDrive::from_environment(
            environment,
            DriveSetup {
                role: SnapRole::Scale,
                direction: spec.direction,
                drag_origin: spec.drag_origin,
            },
            Self::offset_range(&model, points_per_unit),
            initial_position * points_per_unit,
        )?;
        let layout = Self::layout_for(&model, &spec, points_per_unit);
        drive.set_snap_points(&layout.snap_points, now)?;
        Ok(Self {
            last_value: model.value_at(initial_position),
            model,
            spec,
            points_per_unit,
            magnet_capture_points: environment.tokens.density.magnet_capture(spec.density),
            haptics: environment.tokens.haptics,
            drive,
            detents: layout.detents,
            reference_positions: layout.reference_positions,
            last_position: initial_position,
            last_phase: MotionPhase::Resting,
            origin: EventOrigin::Program,
            events: EventQueue::new(),
        })
    }

    /// Включить магнит нуля под пальцем ([`ZeroMagnet`]). `Off` ничего не меняет у любой модели; зона у
    /// плавной модели — ошибка ([`ScaleError::ZeroMagnetRequiresSteppedModel`]), шкала при ошибке теряется.
    pub fn with_zero_magnet(mut self, magnet: ZeroMagnet) -> Result<Self, ScaleError> {
        // Ноль положения модели лежит в нулевом смещении контента (`смещение = положение × pt_на_единицу`).
        let Some(zone) = magnet.hold_zone(0.0) else {
            return Ok(self);
        };
        if self.model.quantization() != Quantization::Stepped {
            return Err(ScaleError::ZeroMagnetRequiresSteppedModel);
        }
        self.drive.set_hold_zone(Some(zone));
        Ok(self)
    }

    // ---- чтение состояния ----

    /// Модель значений (только чтение: пределы меняются через [`SnapScale::set_limits`]).
    pub fn model(&self) -> &M {
        &self.model
    }

    /// Сколько pt занимает одна единица модели (по токену плотности шкалы). Нужно виду, чтобы
    /// перевести смещение риски в номер шага, не зная токенов плотности.
    pub fn points_per_unit(&self) -> f64 {
        self.points_per_unit
    }

    pub fn spec(&self) -> &ScaleSpec {
        &self.spec
    }

    /// Значение в момент `now`.
    pub fn value(&self, now: MonotonicNanos) -> M::Value {
        self.model.value_at(self.position_at(now))
    }

    pub fn phase(&self, now: MonotonicNanos) -> MotionPhase {
        self.drive.phase(now)
    }

    /// Всё для рисования и проверок за один вызов.
    pub fn snapshot(&self, now: MonotonicNanos) -> ScaleSnapshot<M::Value> {
        let sample = self.drive.sample(now);
        let position_units = sample.offset / self.points_per_unit;
        ScaleSnapshot {
            offset_points: sample.offset,
            position_units,
            value: self.model.value_at(position_units),
            phase: MotionPhase::from_scroll(sample.phase),
        }
    }

    /// Скорость контента, pt/с (знак — по смещению контента). Нужна рисованию и проверкам плавности.
    pub fn velocity_points_per_second(&self, now: MonotonicNanos) -> f64 {
        self.drive.sample(now).velocity
    }

    /// Риски для рисования; при [`TickVisibility::Hidden`] — пусто.
    pub fn marks(&self) -> Vec<ScaleMark> {
        match self.spec.ticks {
            TickVisibility::Hidden => Vec::new(),
            TickVisibility::Visible => self
                .model
                .marks()
                .into_iter()
                .map(|mark| ScaleMark {
                    offset_points: mark.position * self.points_per_unit,
                    emphasis: mark.emphasis,
                })
                .collect(),
        }
    }

    // ---- намерения жестов ----

    /// Палец коснулся шкалы: движение останавливается на месте, без скачка.
    pub fn catch(&mut self, now: MonotonicNanos) {
        self.origin = EventOrigin::Finger;
        self.drive.catch(now);
        self.observe(now);
    }

    /// Жест перетаскивания распознан.
    pub fn drag_began(&mut self, now: MonotonicNanos, translation: PanTranslation) -> Result<(), ScaleError> {
        self.origin = EventOrigin::Finger;
        self.drive.drag_began(now, translation)?;
        self.observe(now);
        Ok(())
    }

    /// Палец сдвинулся.
    pub fn drag_moved(&mut self, now: MonotonicNanos, translation: PanTranslation) -> Result<(), ScaleError> {
        self.drive.drag_moved(translation)?;
        self.observe(now);
        Ok(())
    }

    /// Палец отпущен со скоростью `velocity` (pt/с, экранные оси).
    pub fn drag_ended(&mut self, now: MonotonicNanos, velocity: Vector) -> Result<(), ScaleError> {
        let outcome = self.drive.drag_ended(velocity, now)?;
        self.apply_magnet(outcome, now)?;
        self.observe(now);
        Ok(())
    }

    /// Жест отменён системой: **то же, что отпускание без скорости**: шкала доезжает до ближайшего
    /// упора без броска, а плавная шкала возле опорного значения притягивается магнитом.
    ///
    /// Почему не «вернуть, как было» (так делает карусель): значение шкалы менялось вживую под
    /// пальцем, камера уже получала его событиями `ValueChanged`; откат к старому значению был бы
    /// ещё одним скачком выдержки/зума. Мягко доехать до упора от текущего положения — меньшее зло.
    /// У карусели выбор происходит только при отпускании, поэтому ей откатывать нечего лишнего.
    pub fn drag_cancelled(&mut self, now: MonotonicNanos) -> Result<(), ScaleError> {
        let outcome = self.drive.drag_cancelled(now)?;
        self.apply_magnet(outcome, now)?;
        self.observe(now);
        Ok(())
    }

    /// Единая точка входа для пана: событие арены (через [`PanUpdate::from_gesture`]) в нужное
    /// намерение. `time` — время **события** (`GestureEvent::time`); шкале оно нужно на каждом шаге
    /// (события и значение считаются на этот момент).
    pub fn apply_pan(&mut self, time: MonotonicNanos, update: PanUpdate) -> Result<(), ScaleError> {
        match update {
            PanUpdate::Began { translation } => self.drag_began(time, translation),
            PanUpdate::Changed { translation } => self.drag_moved(time, translation),
            PanUpdate::Ended { velocity } => self.drag_ended(time, velocity),
            PanUpdate::Cancelled => self.drag_cancelled(time),
        }
    }

    /// Палец, поймавший шкалу, отпущен без перетаскивания.
    pub fn release_without_drag(&mut self, now: MonotonicNanos) -> Result<(), ScaleError> {
        let outcome = self.drive.release_held(now)?;
        self.apply_magnet(outcome, now)?;
        self.observe(now);
        Ok(())
    }

    // ---- программные изменения ----

    /// Установить значение программно (сброс в ноль, смена камеры). Значение вне пределов или
    /// между шагами заменяется ближайшим допустимым ([`SetValueOutcome::Adjusted`]).
    /// Прерывает перетаскивание. Вибрации нет.
    pub fn set_value(
        &mut self,
        value: M::Value,
        transition: ScrollTransition,
        now: MonotonicNanos,
    ) -> Result<SetValueOutcome<M::Value>, ScaleError> {
        let applied = self.model.clamp_value(value)?;
        let target_offset = self.model.position_of(applied)? * self.points_per_unit;
        self.origin = EventOrigin::Program;
        self.move_to(target_offset, transition, now)?;
        Ok(if applied == value {
            SetValueOutcome::Applied
        } else {
            SetValueOutcome::Adjusted { applied }
        })
    }

    /// Показать шкалу ровно на `position_units` (в единицах модели, не округляя до шага). Нужна, когда значение
    /// пришло снаружи и лежит МЕЖДУ шагами (пресет с 0,15 при шаге 0,1): `set_value` поставил бы его на шаг и
    /// соврал бы о положении. Положение зажимается в пределы модели. Значение, которое сообщит шкала,
    /// по-прежнему ступенчатое; событие `ValueChanged` рождается только при смене ступени, и это движение —
    /// программное (хаптики нет, у экрана оно уходит в эхо).
    pub fn show_position(
        &mut self,
        position_units: f64,
        transition: ScrollTransition,
        now: MonotonicNanos,
    ) -> Result<(), ScaleError> {
        let (lowest, highest) = self.model.position_range();
        let target_offset = position_units.clamp(lowest, highest) * self.points_per_unit;
        self.origin = EventOrigin::Program;
        self.move_to(target_offset, transition, now)
    }

    /// Положение шкалы в единицах модели с учётом незавершённого движения (без округления до шага).
    pub fn position_units(&self, now: MonotonicNanos) -> f64 {
        self.position_at(now)
    }

    /// Сменить пределы модели (другая камера). Положение значений не меняется. Если текущее
    /// положение оказалось вне новых пределов, значение немедленно становится крайним допустимым
    /// (событие `ValueChanged`), а сама шкала `Animated` — доезжает пружиной, `Immediate` — прыгает.
    ///
    /// **Атомарно:** сначала модель, рубежи, точки и привод пересчитываются на копиях, и только
    /// когда всё удалось, шкала заменяет себя целиком. При любой ошибке (модель отвергла пределы,
    /// движок отверг сетку) не меняется ни модель, ни привод, ни события.
    pub fn set_limits(
        &mut self,
        limits: M::Limits,
        transition: ScrollTransition,
        now: MonotonicNanos,
    ) -> Result<(), ScaleError> {
        let mut next_model = self.model.clone();
        next_model.set_limits(limits)?;
        let layout = Self::layout_for(&next_model, &self.spec, self.points_per_unit);
        let mut next_drive = self.drive.clone();
        next_drive.reconfigure(
            Self::offset_range(&next_model, self.points_per_unit),
            &layout.snap_points,
            now,
        )?;
        let mut jumped_into_limits = false;
        if transition == ScrollTransition::Immediate {
            let (lowest, highest) = next_model.position_range();
            let position = next_drive.sample(now).offset / self.points_per_unit;
            let clamped = position.clamp(lowest, highest);
            if clamped != position {
                next_drive.scroll_to(clamped * self.points_per_unit, ScrollTransition::Immediate, now)?;
                jumped_into_limits = true;
            }
        }

        // Всё посчитано и проверено: применяем без возможности ошибки.
        self.model = next_model;
        self.detents = layout.detents;
        self.reference_positions = layout.reference_positions;
        self.drive = next_drive;
        self.origin = EventOrigin::Program;
        if jumped_into_limits {
            // Прыжок без рубежей по пути: наблюдение переносится на новое положение.
            self.last_position = self.position_at(now);
        }
        self.observe(now);
        Ok(())
    }

    /// Предпочтение движения шкалы.
    pub fn motion(&self) -> MotionPreference {
        self.drive.motion()
    }

    /// Сменить предпочтение движения на лету (Reduce Motion включили при открытой шкале).
    /// Действует на новые движения (прилипание, возврат, программная прокрутка); уже идущее
    /// доезжает как было, положение и значение не меняются. Размер области, токены и плотность
    /// на лету не меняются: для них шкалу создают заново.
    pub fn set_motion(&mut self, motion: MotionPreference) {
        self.drive.set_motion(motion);
    }

    // ---- кадр и события ----

    /// Раз в кадр: сравнить положение с прошлым кадром и породить события.
    pub fn advance(&mut self, now: MonotonicNanos) {
        self.observe(now);
    }

    /// Забрать накопленные события (очередь очищается).
    pub fn drain_events(&mut self) -> Vec<ScaleEvent<M::Value>> {
        self.events.drain()
    }

    // ---- внутреннее ----

    fn position_at(&self, now: MonotonicNanos) -> f64 {
        self.drive.sample(now).offset / self.points_per_unit
    }

    /// Отрезок смещений контента: пределы модели в pt.
    fn offset_range(model: &M, points_per_unit: f64) -> OffsetRange {
        let (lowest, highest) = model.position_range();
        OffsetRange {
            min: lowest * points_per_unit,
            max: highest * points_per_unit,
        }
    }

    /// Рубежи, опорные положения и точки прилипания по текущим рискам модели (ничего не меняет).
    fn layout_for(model: &M, spec: &ScaleSpec, points_per_unit: f64) -> MarkLayout {
        let marks = model.marks();
        let snap_points = match spec.snap {
            SnapPolicy::EveryStep => marks.iter().map(|mark| mark.position * points_per_unit).collect(),
            SnapPolicy::ContinuousWithMagnets => Vec::new(),
        };
        MarkLayout {
            detents: build_detents(&marks, model.quantization()),
            reference_positions: marks
                .iter()
                .filter(|mark| mark.emphasis == Emphasis::Reference)
                .map(|mark| mark.position)
                .collect(),
            snap_points,
        }
    }

    /// Программный сдвиг к `target_offset`. `Immediate` — без рубежей по пути: наблюдение
    /// переносится на новое положение до того, как события будут посчитаны.
    fn move_to(
        &mut self,
        target_offset: f64,
        transition: ScrollTransition,
        now: MonotonicNanos,
    ) -> Result<(), ScaleError> {
        match transition {
            ScrollTransition::Animated => self.drive.scroll_to(target_offset, ScrollTransition::Animated, now)?,
            ScrollTransition::Immediate => {
                self.drive.scroll_to(target_offset, ScrollTransition::Immediate, now)?;
                self.last_position = self.position_at(now);
            }
        }
        self.observe(now);
        Ok(())
    }

    /// Магнит: плавная шкала, которая остановится рядом с опорным значением, доезжает до него.
    /// Скорость контента наследуется (пружина стартует с текущей скоростью), ступеньки нет.
    fn apply_magnet(&mut self, outcome: ReleaseOutcome, now: MonotonicNanos) -> Result<(), ScaleError> {
        if self.spec.snap != SnapPolicy::ContinuousWithMagnets {
            return Ok(());
        }
        let resting_offset = match outcome {
            ReleaseOutcome::Coasting { projected_offset } => projected_offset,
            ReleaseOutcome::Settled { offset } => offset,
            // Прилипание к точке (их у магнитной шкалы нет), край и возврат от края — не наше дело.
            ReleaseOutcome::SnapsToPoint { .. }
            | ReleaseOutcome::CoastsPastEdge { .. }
            | ReleaseOutcome::ReturnsFromOverscroll { .. } => return Ok(()),
        };
        let nearest = self
            .reference_positions
            .iter()
            .map(|position| position * self.points_per_unit)
            .map(|offset| (offset, (offset - resting_offset).abs()))
            .filter(|(_, distance)| *distance <= self.magnet_capture_points)
            .min_by(|left, right| left.1.total_cmp(&right.1));
        if let Some((anchor_offset, _)) = nearest {
            self.drive.scroll_to(anchor_offset, ScrollTransition::Animated, now)?;
        }
        Ok(())
    }

    /// Сравнить положение `now` с прошлым наблюдением и записать события.
    fn observe(&mut self, now: MonotonicNanos) {
        let sample = self.drive.sample(now);
        let position = sample.offset / self.points_per_unit;
        let phase = MotionPhase::from_scroll(sample.phase);

        let crossings = crossed(&self.detents, self.last_position, position);
        for (emphasis, direction) in &crossings {
            self.events.push(match emphasis {
                Emphasis::Tick => ScaleEvent::TickCrossed { direction: *direction },
                Emphasis::Reference => ScaleEvent::ReferenceCrossed { direction: *direction },
                // Шаг без риски рубеж не объявляет: ни события, ни щелчка.
                Emphasis::Step => continue,
            });
        }
        let value = self.model.value_at(position);
        if value != self.last_value {
            self.events.push(ScaleEvent::ValueChanged { value });
        }
        let haptic = self.haptic_for(&crossings);
        self.events.push_haptic(self.origin, haptic);
        if phase == MotionPhase::Resting && self.last_phase != MotionPhase::Resting {
            self.events.push(ScaleEvent::Settled { value });
        }
        self.last_position = position;
        self.last_value = value;
        self.last_phase = phase;
    }

    /// Паттерн на кадр: сильнейший из пройденных рубежей (опорное значение сильнее тика). Играть ли
    /// его, решает очередь по происхождению движения (программное не вибрирует).
    fn haptic_for(&self, crossings: &[(Emphasis, TravelDirection)]) -> Option<shell_contract::haptics::HapticPattern> {
        let event = if crossings.iter().any(|(emphasis, _)| *emphasis == Emphasis::Reference) {
            HapticEvent::ScaleReference
        } else if crossings.iter().any(|(emphasis, _)| *emphasis == Emphasis::Tick) {
            HapticEvent::ScaleTick
        } else {
            // Ничего не пройдено (или только шаги без риски): щелчка нет.
            return None;
        };
        self.haptics.pattern_for(event)
    }
}
