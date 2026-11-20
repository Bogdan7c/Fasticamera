//! Факты об объективах для экрана камеры (сессия 13): перевод [`LensStatus`] движка в нейтральные [`LensFacts`].
//!
//! Экран камеры не знает `camera-core`, поэтому здесь — единственное место, где роли и счётчики движка превращаются
//! в язык экрана (камеры, направление, номер неудачи).
//!
//! # Как определяется «последняя неудача» (`LensFacts::last_failure`)
//! Экран отличает НОВУЮ неудачу смены объектива от уже учтённой по номеру `sequence`, и откат показанного
//! коэффициента делает один раз на номер. Поэтому номер обязан **строго расти** на каждой неудаче и **не повторяться**,
//! даже когда статус движка перечитывают много раз. Источник номера — счётчик движка
//! `LensCounters::switches_failed` (растёт ровно на единицу за неудачу, не уменьшается). Цель неудачи — камера роли,
//! которую движок заблокировал ([`LensStatus::blocked`]); блокировку он снимает, когда намерение выбирает другую
//! роль, поэтому цель запоминается в момент, когда счётчик вырос, и живёт в [`LensFactsBuilder`] до следующей неудачи.
//! Первое наблюдение статуса — **точка отсчёта**: неудачи, случившиеся до того, как этот строитель (то есть экран)
//! появился, экрану не «новые» (экран переустанавливается, а счётчик движка живёт весь процесс), поэтому откат
//! показанного значения они не запускают.
//! Если блокировки на этот момент не нашлось (роль не из каталога), неудача не теряется молча: номер растёт, цель —
//! активная камера (экрану нужен именно откат к рабочему объективу), а в [`LensFactsDiagnostics`] идёт счётчик.

use camera_contract::ids::{CameraId, LensRole};
use camera_contract::zoom::ZoomCatalog;
use camera_core::lens_status::{LensStatus, LensSwitchState};
use camera_screen::{ActiveLens, LensFacts, LensSwitch, LensSwitchFailure};

/// Что пошло не так при переводе (уходит в отчёт экрана, не в тишину).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LensFactsDiagnostics {
    /// Неудач смены, у которых цель не удалось найти по заблокированной роли (взята активная камера).
    pub failure_target_unresolved: u64,
    /// Неудач смены, у которых не нашлось даже активной камеры: экрану они не переданы.
    pub failures_without_any_target: u64,
}

/// Строитель фактов об объективах: помнит последнюю неудачу между вызовами (см. доку модуля).
#[derive(Debug, Default)]
pub struct LensFactsBuilder {
    /// Сколько неудач движка уже учтено (значение `switches_failed` при последнем вызове); `None` — ещё не смотрели.
    failures_seen: Option<u64>,
    last_failure: Option<LensSwitchFailure>,
    diagnostics: LensFactsDiagnostics,
}

impl LensFactsBuilder {
    pub fn diagnostics(&self) -> LensFactsDiagnostics {
        self.diagnostics
    }

    /// Факты об объективах по статусу движка. Вызывается на каждом кадре UI; результат меняется только вместе со
    /// статусом.
    pub fn facts_of(&mut self, status: &LensStatus) -> LensFacts {
        self.observe_failures(status);
        LensFacts {
            catalog: catalog_if_published(status),
            active: status.active.as_ref().map(|active| ActiveLens {
                camera: active.camera.clone(),
                role: active.role,
                facing: active.facing,
            }),
            switch: switch_of(status),
            last_failure: self.last_failure.clone(),
        }
    }

    /// Если счётчик неудач вырос — запомнить новую неудачу с номером = значению счётчика.
    fn observe_failures(&mut self, status: &LensStatus) {
        let total = status.counters.switches_failed;
        let Some(seen) = self.failures_seen.replace(total) else {
            // Первое наблюдение: прежние неудачи — история, а не событие для этого экрана.
            return;
        };
        if total <= seen {
            return;
        }
        let blocked_target = status
            .blocked
            .as_ref()
            .and_then(|block| camera_of_role(&status.zoom_catalog, block.role));
        let target = blocked_target.or_else(|| {
            self.diagnostics.failure_target_unresolved += 1;
            status.active.as_ref().map(|active| active.camera.clone())
        });
        match target {
            Some(target) => {
                self.last_failure = Some(LensSwitchFailure {
                    sequence: total,
                    target,
                });
            }
            None => self.diagnostics.failures_without_any_target += 1,
        }
    }
}

/// Каталог — только когда в нём есть хоть одна шкала (пустой начальный каталог движка экрану не показывают).
fn catalog_if_published(status: &LensStatus) -> Option<std::sync::Arc<ZoomCatalog>> {
    let catalog = &status.zoom_catalog;
    (catalog.back.is_some() || catalog.front.is_some()).then(|| std::sync::Arc::clone(catalog))
}

/// Идёт ли смена: `Switching` движка с ролями → камеры. Роль, которой нет в каталоге, смену не показывает
/// (экрану нечем её описать, а картинка размывается по рендеру, не по этому факту).
fn switch_of(status: &LensStatus) -> LensSwitch {
    let LensSwitchState::Switching { from, to } = status.switch else {
        return LensSwitch::Steady;
    };
    let catalog = &status.zoom_catalog;
    match (camera_of_role(catalog, from), camera_of_role(catalog, to)) {
        (Some(from), Some(to)) => LensSwitch::Switching { from, to },
        _ => LensSwitch::Steady,
    }
}

/// Камера роли по каталогу зума (роли уникальны между направлениями: `Front` бывает только у фронтальной группы).
fn camera_of_role(catalog: &ZoomCatalog, role: LensRole) -> Option<CameraId> {
    [catalog.back.as_ref(), catalog.front.as_ref()]
        .into_iter()
        .flatten()
        .find_map(|group| group.lens_for_role(role))
        .map(|lens| lens.camera.clone())
}

#[cfg(test)]
#[path = "camera_screen_lens_facts_tests.rs"]
mod tests;
