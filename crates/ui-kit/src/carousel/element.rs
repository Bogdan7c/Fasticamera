//! K4 «Карусель с прилипанием»: список данных + движок прокрутки с политикой `AdjacentOnly`.
//!
//! # Когда приходит выбор
//! Выбор (событие и хаптика `SelectionTick`) происходит **в момент решения**: при отпускании
//! пальца (движок выбрал целевой элемент) или при тапе, а не по прибытии пружины. Причины:
//! 1) `haptics.md` — selection-хаптика сопровождает смену значения («feedback while the values
//!    of a UI element are changing»); щелчок спустя ≈ 0,85 с после отпускания (длительность
//!    пружины `camera.snap`) ощущался бы запоздавшим и оторванным от жеста;
//! 2) режим переключается сразу, камере не нужно ждать окончания анимации (решение 80/М1).
//!
//! `selected()` меняется в тот же момент; положение контента догоняет его пружиной.
//!
//! # Одна точка и пустой список
//! Из одного элемента карусель тянется только «резинкой»: точка одна, выбор не меняется. Пустой
//! список — ошибка ([`CarouselError::EmptyEntries`]).

use std::fmt::Debug;

use ui_core::geometry::Vector;
use ui_core::gesture::PanTranslation;
use ui_core::scroll::ReleaseOutcome;
use ui_core::time::{MonotonicNanos, MotionPreference};

use super::entry::{CarouselEntry, CarouselSpec, ItemPlacement, snap_offsets};
use super::error::CarouselError;
use super::events::{CarouselEvent, SelectionCause, TapOutcome};
use crate::drive::{AxisDrive, DriveSetup, MotionPhase, OffsetRange, ScrollTransition};
use crate::environment::ElementEnvironment;
use crate::event_queue::EventQueue;
use crate::tokens::{HapticEvent, HapticTable, SnapRole};

/// Карусель элементов `T`. Идентичность элемента — `T: PartialEq`.
pub struct SnapCarousel<T> {
    entries: Vec<CarouselEntry<T>>,
    /// Смещения точек прилипания (центры элементов относительно первого), pt.
    snap_offsets: Vec<f64>,
    selected: usize,
    gap_points: f64,
    haptics: HapticTable,
    drive: AxisDrive,
    /// Свайп начался и ещё не закончился: капсула расширена (`behavior.md` 2.4).
    swipe_in_progress: bool,
    events: EventQueue<CarouselEvent<T>>,
}

impl<T: Clone + PartialEq + Debug> SnapCarousel<T> {
    /// Создать карусель в покое на элементе `selected`.
    pub fn new(
        entries: Vec<CarouselEntry<T>>,
        selected: &T,
        spec: CarouselSpec,
        environment: &ElementEnvironment<'_>,
        now: MonotonicNanos,
    ) -> Result<Self, CarouselError> {
        let gap_points = environment.tokens.density.carousel_item_gap_points;
        let offsets = snap_offsets(&entries, gap_points)?;
        let selected_index = Self::index_of(&entries, selected).ok_or(CarouselError::UnknownItem)?;
        let initial_offset = Self::offset_of(&offsets, selected_index)?;
        let mut drive = AxisDrive::from_environment(
            environment,
            DriveSetup {
                role: SnapRole::Carousel,
                direction: spec.direction,
                drag_origin: spec.drag_origin,
            },
            Self::offset_range(&offsets),
            initial_offset,
        )?;
        drive.set_snap_points(&offsets, now)?;
        Ok(Self {
            entries,
            snap_offsets: offsets,
            selected: selected_index,
            gap_points,
            haptics: environment.tokens.haptics,
            drive,
            swipe_in_progress: false,
            events: EventQueue::new(),
        })
    }

    // ---- чтение состояния ----

    pub fn entries(&self) -> &[CarouselEntry<T>] {
        &self.entries
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// Выбранный элемент (меняется в момент решения, см. документацию модуля).
    pub fn selected(&self) -> &T {
        &self.entries[self.selected].item
    }

    /// Смещения точек прилипания, pt.
    pub fn snap_offsets(&self) -> &[f64] {
        &self.snap_offsets
    }

    /// Смещение контента, pt (0 — первый элемент по центру).
    pub fn offset_points(&self, now: MonotonicNanos) -> f64 {
        self.drive.sample(now).offset
    }

    /// Скорость контента, pt/с.
    pub fn velocity_points_per_second(&self, now: MonotonicNanos) -> f64 {
        self.drive.sample(now).velocity
    }

    pub fn phase(&self, now: MonotonicNanos) -> MotionPhase {
        self.drive.phase(now)
    }

    /// Капсула расширена: от начала перетаскивания до остановки карусели (`behavior.md` 2.4:
    /// «капсула кратко расширяется, когда начинаешь свайп»). Тап и программный выбор капсулу не расширяют.
    pub fn capsule_expanded(&self, now: MonotonicNanos) -> bool {
        self.swipe_in_progress && self.phase(now) != MotionPhase::Resting
    }

    /// Где стоят элементы сейчас.
    pub fn placements(&self, now: MonotonicNanos) -> Vec<ItemPlacement> {
        let offset = self.offset_points(now);
        self.entries
            .iter()
            .zip(&self.snap_offsets)
            .enumerate()
            .map(|(index, (entry, snap_offset))| ItemPlacement {
                index,
                center_offset_points: snap_offset - offset,
                width_points: entry.width_points,
            })
            .collect()
    }

    // ---- намерения жестов ----

    /// Палец коснулся карусели: движение останавливается на месте, без скачка.
    pub fn catch(&mut self, now: MonotonicNanos) {
        self.swipe_in_progress = false;
        self.drive.catch(now);
    }

    /// Жест перетаскивания распознан.
    pub fn drag_began(&mut self, now: MonotonicNanos, translation: PanTranslation) -> Result<(), CarouselError> {
        self.drive.drag_began(now, translation)?;
        self.swipe_in_progress = true;
        Ok(())
    }

    /// Палец сдвинулся.
    pub fn drag_moved(&mut self, translation: PanTranslation) -> Result<(), CarouselError> {
        self.drive.drag_moved(translation)?;
        Ok(())
    }

    /// Палец отпущен со скоростью `velocity` (pt/с, экранные оси): выбирается соседний элемент.
    pub fn drag_ended(&mut self, now: MonotonicNanos, velocity: Vector) -> Result<(), CarouselError> {
        let outcome = self.drive.drag_ended(velocity, now)?;
        self.commit_release(outcome, SelectionCause::Swipe);
        Ok(())
    }

    /// Жест отменён системой: **то же, что отпускание без скорости** (одно правило для всех
    /// элементов): фиксируется ближайший элемент (соседний по политике `AdjacentOnly`) и приходит
    /// событие выбора, если он сменился. Причина выбора — `Swipe` (палец начал жест).
    pub fn drag_cancelled(&mut self, now: MonotonicNanos) -> Result<(), CarouselError> {
        let outcome = self.drive.drag_cancelled(now)?;
        self.commit_release(outcome, SelectionCause::Swipe);
        Ok(())
    }

    /// Палец, поймавший карусель, отпущен без перетаскивания.
    pub fn release_without_drag(&mut self, now: MonotonicNanos) -> Result<(), CarouselError> {
        let outcome = self.drive.release_held(now)?;
        self.commit_release(outcome, SelectionCause::Swipe);
        Ok(())
    }

    /// Тап по элементу `index`: карусель едет к нему анимированно.
    pub fn tapped_item(&mut self, index: usize, now: MonotonicNanos) -> Result<TapOutcome, CarouselError> {
        let target_offset = self.offset_for_index(index)?;
        self.swipe_in_progress = false;
        self.drive.scroll_to(target_offset, ScrollTransition::Animated, now)?;
        if index == self.selected {
            return Ok(TapOutcome::AlreadySelected);
        }
        self.commit_selection(index, SelectionCause::Tap);
        Ok(TapOutcome::Selected)
    }

    // ---- программные изменения ----

    /// Выбрать элемент программно (состояние камеры изменилось само). Вибрации нет.
    pub fn select(
        &mut self,
        index: usize,
        transition: ScrollTransition,
        now: MonotonicNanos,
    ) -> Result<(), CarouselError> {
        let target_offset = self.offset_for_index(index)?;
        self.swipe_in_progress = false;
        self.drive.scroll_to(target_offset, transition, now)?;
        self.commit_selection(index, SelectionCause::Programmatic);
        Ok(())
    }

    /// Предпочтение движения карусели.
    pub fn motion(&self) -> MotionPreference {
        self.drive.motion()
    }

    /// Сменить предпочтение движения на лету (Reduce Motion включили при открытой карусели).
    /// Действует на новые движения; идущее доезжает как было, выбор и положение не меняются.
    /// Токены, плотность и размер области на лету не меняются: для них карусель создают заново.
    pub fn set_motion(&mut self, motion: MotionPreference) {
        self.drive.set_motion(motion);
    }

    /// Заменить список на лету (добавили режим). Выбранный элемент сохраняется **по идентичности**;
    /// если его в новом списке нет, выбирается элемент на том же месте (или последний) и приходит
    /// `SelectionChanged` с `Programmatic`. Покоящаяся карусель переставляется сразу, едущая —
    /// доезжает пружиной; под пальцем контент не трогаем.
    ///
    /// **Атомарно:** список, точки и привод считаются на копии; карусель заменяется целиком, только
    /// когда всё удалось. При ошибке (пустой список, плохая ширина, отказ движка) не меняются ни
    /// список, ни выбор, ни привод, ни события.
    pub fn set_entries(&mut self, entries: Vec<CarouselEntry<T>>, now: MonotonicNanos) -> Result<(), CarouselError> {
        let offsets = snap_offsets(&entries, self.gap_points)?;
        let previous_item = self.selected().clone();
        let found = Self::index_of(&entries, &previous_item);
        let last_index = entries.len().checked_sub(1).ok_or(CarouselError::EmptyEntries)?;
        let new_selected = found.unwrap_or_else(|| self.selected.min(last_index));
        let selected_offset = Self::offset_of(&offsets, new_selected)?;
        // Фаза запоминается до смены границ: сужение границ само запускает возврат пружиной,
        // и покоящуюся карусель после него уже не отличить от едущей.
        let phase_before = self.drive.phase(now);

        let mut next_drive = self.drive.clone();
        next_drive.reconfigure(Self::offset_range(&offsets), &offsets, now)?;
        match phase_before {
            MotionPhase::Touched => {}
            MotionPhase::Resting => next_drive.scroll_to(selected_offset, ScrollTransition::Immediate, now)?,
            MotionPhase::Settling => next_drive.scroll_to(selected_offset, ScrollTransition::Animated, now)?,
        }

        // Всё посчитано и проверено: применяем без возможности ошибки.
        self.drive = next_drive;
        self.entries = entries;
        self.snap_offsets = offsets;
        self.selected = new_selected;
        if found.is_none() {
            self.push_selection_event(SelectionCause::Programmatic);
        }
        Ok(())
    }

    /// Забрать накопленные события.
    pub fn drain_events(&mut self) -> Vec<CarouselEvent<T>> {
        self.events.drain()
    }

    // ---- внутреннее ----

    fn index_of(entries: &[CarouselEntry<T>], item: &T) -> Option<usize> {
        entries.iter().position(|entry| entry.item == *item)
    }

    /// Смещение точки `index` в списке смещений; индекс вне списка — ошибка.
    fn offset_of(offsets: &[f64], index: usize) -> Result<f64, CarouselError> {
        offsets.get(index).copied().ok_or(CarouselError::IndexOutOfRange {
            index,
            length: offsets.len(),
        })
    }

    fn offset_for_index(&self, index: usize) -> Result<f64, CarouselError> {
        Self::offset_of(&self.snap_offsets, index)
    }

    /// Отрезок смещений: от первого элемента до последнего.
    fn offset_range(offsets: &[f64]) -> OffsetRange {
        OffsetRange {
            min: 0.0,
            max: offsets.last().copied().unwrap_or(0.0),
        }
    }

    /// Итог отпускания → выбор. И `SnapsToPoint` (свайп внутри границ), и возврат из-за края
    /// (карусель перетянута за последний элемент) приводят к элементу, к которому поедет контент.
    fn commit_release(&mut self, outcome: ReleaseOutcome, cause: SelectionCause) {
        let target_index = match outcome {
            ReleaseOutcome::SnapsToPoint { index, .. } => Some(index),
            // Индекс точки возврата приходит готовым от движка: точку не ищут сравнением чисел.
            ReleaseOutcome::ReturnsFromOverscroll { snap_index, .. } => snap_index,
            // Точки у карусели есть всегда, поэтому свободный бросок и покой вне точки невозможны.
            ReleaseOutcome::Settled { .. }
            | ReleaseOutcome::Coasting { .. }
            | ReleaseOutcome::CoastsPastEdge { .. } => None,
        };
        if let Some(index) = target_index {
            self.commit_selection(index, cause);
        }
    }

    /// Сменить выбор; ничего не делает, если элемент уже выбран (ровно одно событие на смену).
    fn commit_selection(&mut self, index: usize, cause: SelectionCause) {
        if index == self.selected {
            return;
        }
        self.selected = index;
        self.push_selection_event(cause);
    }

    fn push_selection_event(&mut self, cause: SelectionCause) {
        self.events.push(CarouselEvent::SelectionChanged {
            item: self.selected().clone(),
            index: self.selected,
            cause,
        });
        // Хаптика — только если выбор сделал палец: решает очередь по происхождению причины.
        let haptic = self.haptics.pattern_for(HapticEvent::CarouselSelection);
        self.events.push_haptic(cause.origin(), haptic);
    }
}
