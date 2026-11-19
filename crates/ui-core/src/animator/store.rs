//! [`PropertyAnimator`]: хранилище анимаций свойств и их применение к дереву.

use std::collections::BTreeMap;

use super::config::AnimatorConfig;
use super::error::AnimatorError;
use super::property::{PropertyKey, ValueRange, read_property, write_property};
use crate::id::NodeId;
use crate::spring::{Spring, SpringAnimation, SpringPhase, SpringSample, SpringStart};
use crate::time::{MonotonicNanos, MotionPreference};
use crate::tree::{TreeError, UiTree};

/// С какой скоростью начинается анимация свойства.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StartVelocity {
    /// С нуля (кнопку нажали, пальца нет).
    Zero,
    /// Унаследовать скорость уже едущей анимации этого свойства (прерывание как в iOS); если
    /// свойство не едет — с нуля.
    Inherit,
    /// Явная скорость в единицах свойства в секунду (скорость пальца при отпускании).
    Explicit(f64),
}

/// Просьба анимировать свойство. Поля именованные: позиционных аргументов нет.
#[derive(Debug, Clone, Copy)]
pub struct AnimationRequest {
    /// Какое свойство какого элемента.
    pub key: PropertyKey,
    /// Цель; должна лежать в допустимом диапазоне свойства.
    pub target: f64,
    /// Пружина (при Reduce Motion `bounce > 0` заменяется на 0).
    pub spring: Spring,
    pub velocity: StartVelocity,
    /// Момент начала (или перенацеливания). Может быть новее последнего кадра — например, время
    /// события отпускания пальца; до этого момента значение стоит на месте.
    pub now: MonotonicNanos,
}

/// Что сделал [`PropertyAnimator::animate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationStart {
    /// Свойство не ехало — новая анимация от текущего значения в дереве.
    Started,
    /// Свойство уже ехало — оно перенацелено от текущего положения анимации.
    Retargeted,
}

/// Результат [`PropertyAnimator::snap`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapOutcome {
    /// Значение установлено, анимации не было.
    Applied,
    /// Значение установлено, ехавшая анимация снята.
    AppliedAndCancelledAnimation,
}

/// Результат [`PropertyAnimator::freeze`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FreezeOutcome {
    /// Свойство ехало: анимация снята, свойство осталось там, где его видно на экране.
    Frozen { value: f64 },
    /// Свойство и так стояло: ничего не менялось.
    NotAnimating { value: f64 },
}

/// Значение, которое дерево отвергло при применении (не должно случаться после обрезки; если
/// случилось — это видно, а не теряется).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RejectedValue {
    pub key: PropertyKey,
    pub error: TreeError,
}

/// Отчёт одного шага [`PropertyAnimator::advance`]. Счётчики, а не `bool`: вызывающему важно
/// различать «ничего не делалось», «всё применено» и «часть пропущена».
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AdvanceReport {
    /// Сколько значений записано в дерево (включая финальные значения успокоившихся).
    pub applied: usize,
    /// Сколько анимаций успокоилось на этом шаге (они записаны в `applied` и сняты).
    pub settled: usize,
    /// Сколько значений обрезано до диапазона свойства (перелёт прозрачности и т. п.).
    pub clamped_to_range: usize,
    /// Сколько значений удержано Reduce Motion внутри отрезка «от старта до цели».
    pub clamped_by_reduced_motion: usize,
    /// Сколько анимаций сняты, потому что их элемента уже нет (идентификатор с другим поколением
    /// не применяется к чужому элементу).
    pub dropped_dead_nodes: usize,
    /// Значения, отвергнутые деревом; их анимации сняты.
    pub rejected: Vec<RejectedValue>,
}

/// Отрезок, внутри которого Reduce Motion держит значение.
#[derive(Debug, Clone, Copy)]
struct HoldInterval {
    low: f64,
    high: f64,
}

/// Одна анимация и (при Reduce Motion) отрезок, за который значение выйти не может.
#[derive(Debug, Clone, Copy)]
struct Entry {
    animation: SpringAnimation,
    hold: Option<HoldInterval>,
}

/// Аниматор свойств: владеет анимациями, дерево получает как параметр (оно принадлежит сцене).
///
/// Порядок применения детерминирован: `BTreeMap` по [`PropertyKey`].
#[derive(Debug, Clone)]
pub struct PropertyAnimator {
    config: AnimatorConfig,
    motion: MotionPreference,
    entries: BTreeMap<PropertyKey, Entry>,
}

impl PropertyAnimator {
    pub fn new(config: AnimatorConfig, motion: MotionPreference) -> Result<Self, AnimatorError> {
        config.validate()?;
        Ok(Self {
            config,
            motion,
            entries: BTreeMap::new(),
        })
    }

    pub fn motion(&self) -> MotionPreference {
        self.motion
    }

    /// Меняет предпочтение движения. Уже едущие анимации доезжают как были; действует на новые.
    pub fn set_motion(&mut self, motion: MotionPreference) {
        self.motion = motion;
    }

    /// Начать или перенацелить анимацию свойства (прерывание — `motion.md` 1.4).
    ///
    /// При ошибке ничего не меняется (ни анимации, ни дерево).
    pub fn animate(&mut self, tree: &UiTree, request: AnimationRequest) -> Result<AnimationStart, AnimatorError> {
        let key = request.key;
        // Мёртвый элемент (или идентификатор с другим поколением) — отказ, а не анимация «в никуда».
        if !tree.contains(key.node) {
            return Err(TreeError::UnknownNode(key.node).into());
        }
        let range = ValueRange::for_property(key.property, self.config.min_scale);
        if !range.contains(request.target) {
            return Err(AnimatorError::TargetOutOfRange {
                property: key.property,
                value: request.target,
            });
        }
        let spring = request.spring.for_motion(self.motion);
        let rest = self.config.rest_for(key.property);
        // Откуда едем: из положения ехавшей анимации (физического, без обрезки) или из дерева.
        let running = self.entries.get(&key).map(|entry| entry.animation);
        let (current, current_velocity) = match running {
            Some(animation) => {
                let sample = animation.sample(request.now.max(animation.start_time()));
                (sample.value, sample.velocity)
            }
            None => (read_property(tree, key)?, 0.0),
        };
        let initial_velocity = match request.velocity {
            StartVelocity::Zero => 0.0,
            StartVelocity::Inherit => current_velocity,
            StartVelocity::Explicit(velocity) => velocity,
        };
        let animation = SpringAnimation::start(SpringStart {
            spring,
            from: current,
            to: request.target,
            initial_velocity,
            start_time: request.now,
            rest,
        })?;
        let hold = (self.motion == MotionPreference::Reduced).then(|| HoldInterval {
            low: current.min(request.target),
            high: current.max(request.target),
        });
        self.entries.insert(key, Entry { animation, hold });
        Ok(if running.is_some() {
            AnimationStart::Retargeted
        } else {
            AnimationStart::Started
        })
    }

    /// Установить значение сразу, без анимации; ехавшая анимация этого свойства снимается.
    /// Недопустимое значение — ошибка, анимация остаётся.
    pub fn snap(&mut self, tree: &mut UiTree, key: PropertyKey, value: f64) -> Result<SnapOutcome, AnimatorError> {
        let range = ValueRange::for_property(key.property, self.config.min_scale);
        if !range.contains(value) {
            return Err(AnimatorError::TargetOutOfRange {
                property: key.property,
                value,
            });
        }
        write_property(tree, key, value)?;
        Ok(if self.entries.remove(&key).is_some() {
            SnapOutcome::AppliedAndCancelledAnimation
        } else {
            SnapOutcome::Applied
        })
    }

    /// Шаг: применить значения всех анимаций в момент `now` к дереву.
    pub fn advance(&mut self, now: MonotonicNanos, tree: &mut UiTree) -> AdvanceReport {
        let mut report = AdvanceReport::default();
        let keys: Vec<PropertyKey> = self.entries.keys().copied().collect();
        for key in keys {
            let Some(entry) = self.entries.get(&key).copied() else {
                continue;
            };
            if !tree.contains(key.node) {
                self.entries.remove(&key);
                report.dropped_dead_nodes += 1;
                continue;
            }
            let sample = entry.animation.sample(now);
            let outcome = self.apply_sample(tree, key, &entry, sample, &mut report);
            match outcome {
                Ok(()) => {
                    report.applied += 1;
                    if sample.phase == SpringPhase::AtRest {
                        self.entries.remove(&key);
                        report.settled += 1;
                    }
                }
                Err(error) => {
                    self.entries.remove(&key);
                    report.rejected.push(RejectedValue { key, error });
                }
            }
        }
        report
    }

    /// Снять анимации указанных элементов (элементы удалены). Возвращает, сколько снято.
    pub fn forget(&mut self, nodes: &[NodeId]) -> usize {
        let before = self.entries.len();
        self.entries.retain(|key, _| !nodes.contains(&key.node));
        before - self.entries.len()
    }

    /// Едет ли свойство.
    pub fn is_animating(&self, key: PropertyKey) -> bool {
        self.entries.contains_key(&key)
    }

    /// Цель едущей анимации.
    pub fn target(&self, key: PropertyKey) -> Option<f64> {
        self.entries.get(&key).map(|entry| entry.animation.target())
    }

    /// Физические значение и скорость пружины в момент `now` (до обрезки диапазоном); `None`,
    /// если свойство не едет. Нужна, чтобы проверять непрерывность скорости.
    pub fn sample(&self, key: PropertyKey, now: MonotonicNanos) -> Option<SpringSample> {
        self.entries.get(&key).map(|entry| entry.animation.sample(now))
    }

    /// «Схватить» едущее свойство (касание по движущейся панели, `motion.md` 2: «You don't have
    /// to wait»): анимация снимается, свойство остаётся в точке, **которую видно на экране** —
    /// значении, применённом к дереву последним кадром (как presentation layer в UIKit), а не
    /// в физической точке пружины на момент касания: так элемент замирает ровно под пальцем.
    /// Дальше экран ведёт свойство за пальцем 1:1 от этого значения.
    pub fn freeze(&mut self, tree: &UiTree, key: PropertyKey) -> Result<FreezeOutcome, AnimatorError> {
        // Мёртвый id — ошибка дерева, а не «не ехало»: иначе экран вёл бы удалённый элемент.
        let value = read_property(tree, key)?;
        Ok(if self.entries.remove(&key).is_some() {
            FreezeOutcome::Frozen { value }
        } else {
            FreezeOutcome::NotAnimating { value }
        })
    }

    /// Число едущих анимаций.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Записывает значение выборки в дерево, обрезая по диапазону и (Reduce Motion) по отрезку.
    /// Саму пружину не трогает: её скорость остаётся физической.
    fn apply_sample(
        &self,
        tree: &mut UiTree,
        key: PropertyKey,
        entry: &Entry,
        sample: SpringSample,
        report: &mut AdvanceReport,
    ) -> Result<(), TreeError> {
        let range = ValueRange::for_property(key.property, self.config.min_scale);
        let in_range = range.clamp(sample.value);
        let held = entry.hold.map_or(in_range, |hold| in_range.clamp(hold.low, hold.high));
        if in_range != sample.value {
            report.clamped_to_range += 1;
        } else if held != in_range {
            report.clamped_by_reduced_motion += 1;
        }
        write_property(tree, key, held)
    }
}
