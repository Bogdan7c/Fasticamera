//! Фильтр входа «касание принадлежит системе»: касания, начатые в зоне обязательного системного жеста.
//!
//! # Проблема
//! Панель навигации Android скрыта, но окно остаётся интерактивным: свайп от нижнего края, которым
//! владелец вытягивает кнопки, приходит в приложение и обычными касаниями. Без фильтра жест «свайп
//! вверх по видоискателю» открыл бы меню Controls одновременно с системной панелью.
//!
//! # Правило (решение владельца, пакет P8)
//! * Касание, **палец которого опустился** (`Began`) в зоне, принадлежит системе: вся последовательность
//!   этого указателя до `Ended`/`Cancelled` экраном **не обрабатывается** (ни жесты, ни кнопки), со счётчиком.
//! * Касание, начатое вне зоны, работает как раньше, даже если потом заходит в зону.
//! * Остальные указатели решаются каждый сам по своему `Began`.
//! * Разрыв потока касаний ([`SystemGestureFilter::reset_pointers`]) сбрасывает учёт: потерянные `Ended`
//!   не должны оставить указатель «навсегда системным».
//!
//! Это чисто фильтр входа без состояния экрана, поэтому он живёт в фасаде, а не в редьюсере: зоны не
//! влияют на логику экрана, только на то, какие касания до неё доходят.

use shell_contract::surface::SurfaceSize;
use shell_contract::system_gesture::SystemGestureZones;
use shell_contract::touch::{PointerId, TouchPhase, TouchSample};

/// Что делать с образцом касания.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TouchAdmission {
    /// Экран обрабатывает образец как обычно.
    Process,
    /// Образец — начало касания, принадлежащего системе (счётчик касаний растёт на 1).
    IgnoredStart,
    /// Образец — продолжение или конец касания, уже принадлежащего системе.
    IgnoredContinuation,
}

/// Текущие зоны и указатели, отданные системе.
#[derive(Debug, Default)]
pub(super) struct SystemGestureFilter {
    zones: SystemGestureZones,
    /// Указатели, чей `Began` попал в зону; пальцев у телефона единицы, вектор дешевле хеш-таблицы.
    system_owned: Vec<PointerId>,
}

impl SystemGestureFilter {
    /// Новые зоны. Уже идущие касания сохраняют решение, принятое на их `Began`.
    pub(super) fn set_zones(&mut self, zones: SystemGestureZones) {
        self.zones = zones;
    }

    pub(super) fn zones(&self) -> SystemGestureZones {
        self.zones
    }

    /// Забыть все указатели (разрыв потока касаний, смена сцены).
    pub(super) fn reset_pointers(&mut self) {
        self.system_owned.clear();
    }

    /// Решает судьбу образца. `surface` — размер surface экрана; неизвестен — зон нет.
    pub(super) fn admit(&mut self, surface: Option<SurfaceSize>, sample: &TouchSample) -> TouchAdmission {
        let owned_index = self.system_owned.iter().position(|pointer| *pointer == sample.pointer);
        match sample.phase {
            TouchPhase::Began => {
                // Новое касание: старая запись (оставшаяся от потерянного `Ended`) устарела.
                if let Some(index) = owned_index {
                    self.system_owned.swap_remove(index);
                }
                let in_zone = surface.is_some_and(|size| self.zones.contains(size, sample.position));
                if in_zone {
                    self.system_owned.push(sample.pointer);
                    TouchAdmission::IgnoredStart
                } else {
                    TouchAdmission::Process
                }
            }
            TouchPhase::Moved => match owned_index {
                Some(_) => TouchAdmission::IgnoredContinuation,
                None => TouchAdmission::Process,
            },
            TouchPhase::Ended | TouchPhase::Cancelled => match owned_index {
                Some(index) => {
                    self.system_owned.swap_remove(index);
                    TouchAdmission::IgnoredContinuation
                }
                None => TouchAdmission::Process,
            },
        }
    }
}
