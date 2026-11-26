//! Жесты уходящего раздела калибровки: их заканчивает сам уходящий раздел (корневая причина «продолжений без начала»).
//!
//! # Проблема
//! Раздел пересобирается внутри `Stage::sync` (поворот телефона, смена размера, смена набора полос). Снятие его узлов
//! (`remove_subtree`) заставляет арену жестов выдать `Cancelled` каждому идущему распознавателю уходящего раздела.
//! Эти события попадают в очередь сцены уже ПОСЛЕ того, как кадр выбрал свои события, и разбираются только в
//! следующем кадре — когда на их месте стоит НОВЫЙ раздел. Маршрутизация идёт по намерению (`CalibrationRulerPan(0)`
//! у старой и новой строки одно и то же), и чужой `Cancelled` падал в новую строку, которая такого жеста не начинала.
//!
//! # Решение
//! Сразу после снятия узлов `Stage` забирает из очереди события, порождённые снятием, и отдаёт события
//! раздела калибровки самому уходящему разделу: его линейки и круг получают свой `Cancelled` (значение остаётся
//! там, куда дошло, фокус пальца заканчивается). Чужие события (не раздела) не теряются: они ждут обычного разбора
//! следующего кадра. Результаты (действия, хаптика, счётчики) копятся здесь и забираются фасадом `Screen`.

use ui_core::gesture::GestureEvent;

use super::diagnostics::GestureCounters;
use super::stage::Stage;
use crate::widget::calibration::CalibrationSection;
use crate::widget::error::WidgetError;
use crate::widget::intent::ScreenGesture;
use crate::widget::kit::SceneKit;
use crate::widget::{Ctx, Outputs};

/// Что уходящие разделы накопили с прошлого забора: выход виджетов и счётчики.
#[derive(Debug, Default)]
pub(crate) struct RetiredGestureReport {
    /// Действия и хаптика, которые уходящий раздел выдал, заканчивая жесты (обычно пусто: отмена «без действия»).
    pub outputs: Outputs,
    /// Жесты, разобранные уходящими разделами (учитываются так же, как разобранные в кадре).
    pub counters: GestureCounters,
    /// События раздела, которых не взял ни один его виджет.
    pub unhandled: u64,
}

/// Владелец жестов, порождённых снятием раздела.
#[derive(Debug, Default)]
pub(super) struct RetiredGestures {
    /// События не раздела, выданные снятием узлов: разбираются обычным путём следующего кадра, ничего не теряется.
    deferred: Vec<GestureEvent<ScreenGesture>>,
    /// Ещё не забранный фасадом итог.
    pending: RetiredGestureReport,
    /// Страховочный счётчик «продолжений пана без начала» у уже убранных разделов (живые считаются у раздела).
    orphan_pan_continuations_of_retired: u64,
    /// То же для отказов «картинки под пальцем» (пустой список ведомых узлов).
    finger_focus_refusals_of_retired: u64,
}

impl RetiredGestures {
    /// Заканчивает жесты только что снятого раздела `section`: события раздела разбирает он сам, остальные
    /// откладываются. Вызывается сразу после `section.retire(kit)`, пока уходящий раздел ещё жив как значение.
    pub(super) fn settle(
        &mut self,
        section: &mut CalibrationSection,
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
    ) -> Result<(), WidgetError> {
        for event in kit.scene_mut().drain_gesture_events() {
            if !event.intent.belongs_to_calibration() {
                self.deferred.push(event);
                continue;
            }
            Stage::count_gesture(&mut self.pending.counters, &event);
            if !section.handle_gesture(kit, ctx, &event, &mut self.pending.outputs)? {
                self.pending.unhandled += 1;
            }
        }
        // Страховочные счётчики умирают вместе с разделом, поэтому их итог забирается здесь.
        self.orphan_pan_continuations_of_retired += section.orphan_pan_continuations();
        self.finger_focus_refusals_of_retired += section.finger_focus_refusals();
        Ok(())
    }

    /// Отложенные события (раньше событий сцены следующего разбора): порядок событий сохраняется.
    pub(super) fn take_deferred(&mut self) -> Vec<GestureEvent<ScreenGesture>> {
        std::mem::take(&mut self.deferred)
    }

    /// Забрать накопленный итог (выход виджетов и счётчики).
    pub(super) fn take_report(&mut self) -> RetiredGestureReport {
        std::mem::take(&mut self.pending)
    }

    pub(super) fn orphan_pan_continuations_of_retired(&self) -> u64 {
        self.orphan_pan_continuations_of_retired
    }

    pub(super) fn finger_focus_refusals_of_retired(&self) -> u64 {
        self.finger_focus_refusals_of_retired
    }
}
