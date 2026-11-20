//! План сценария `zoom` (сессия 13): список шагов, которые сценарий выполняет на настоящем экране камеры.
//!
//! Шаг — одно намерение пользователя: тап по кнопке ленты зума, переворот камеры, протяжка пальцем по ленте. Что на
//! каждом шаге должно случиться с объективом (сменится ли камера и какая), решает [`SwitchExpectation`] по состоянию
//! движка В НАЧАЛЕ шага ([`expectation_for_tap`]): 2× — кроп основной камеры, и тап на него с основной камеры
//! объектив не меняет, а тот же тап с ширика — меняет. План от этого не зависит: он только перечисляет действия.
//!
//! Порядок по умолчанию: тапы `.6, 1, 2, 1, .6` → переворот на фронталку → переворот назад (экран возвращает последнее
//! заднее значение — `.6`) → тап `1` (чтобы протяжка шла с основной камеры и не пересекала упор ширика) → протяжка по
//! ленте (хаптика) → завершающий тап `1`: сценарий оставляет экран на 1× основной камеры, как нашёл.

use camera_contract::ids::{LensFacing, LensRole};
use camera_contract::zoom::{LensGroup, ZoomStopKind};

use crate::zoom_scenario_config::ZoomScenarioConfig;

/// Что делает шаг с экраном.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ZoomAction {
    /// Тап по кнопке ленты, ближайшей к этому коэффициенту (действие `ZoomStopTapped`).
    TapStop { ratio: f32 },
    /// Кнопка переворота камеры (действие `FlipCameraRequested`).
    Flip,
    /// Протяжка пальцем по ленте: открывает колесо и ведёт значение (настоящие образцы касаний).
    WheelDrag,
}

/// Один шаг плана.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoomStep {
    /// Имя для отчёта: номер и действие (`01_tap_0.6`).
    pub name: String,
    pub action: ZoomAction,
}

/// Что движок обязан сделать с объективом после шага.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchExpectation {
    /// Объектив остаётся тем же: события `LensSwitched` быть не должно.
    NoSwitch,
    /// Смена на камеру этой роли.
    ToRole(LensRole),
    /// Смена на любой задний объектив (экран сам выбирает, какой: возврат к последнему заднему значению).
    ToAnyBack,
}

impl SwitchExpectation {
    /// Подходит ли завершённая смена на роль `to` под ожидание.
    pub fn accepts(self, to: LensRole) -> bool {
        match self {
            Self::NoSwitch => false,
            Self::ToRole(expected) => expected == to,
            Self::ToAnyBack => facing_of_role(to) == LensFacing::Back,
        }
    }

    /// Ждём ли вообще смену объектива.
    pub fn expects_switch(self) -> bool {
        self != Self::NoSwitch
    }

    /// Какое направление должны показывать кадры после смены (для проверки зеркала).
    pub fn facing_after(self, current: Option<LensFacing>) -> Option<LensFacing> {
        match self {
            Self::NoSwitch => current,
            Self::ToRole(role) => Some(facing_of_role(role)),
            Self::ToAnyBack => Some(LensFacing::Back),
        }
    }
}

/// Направление роли: `Front` — фронтальная, остальные — задние (внешних камер у продукта нет).
pub fn facing_of_role(role: LensRole) -> LensFacing {
    match role {
        LensRole::Front => LensFacing::Front,
        LensRole::UltraWide | LensRole::Wide | LensRole::Tele => LensFacing::Back,
    }
}

/// Список шагов по конфигу.
pub fn planned_steps(config: &ZoomScenarioConfig) -> Vec<ZoomStep> {
    let mut actions: Vec<(String, ZoomAction)> = Vec::new();
    for ratio in &config.stop_ratios {
        actions.push((format!("tap_{ratio}"), ZoomAction::TapStop { ratio: *ratio }));
    }
    if config.flip_round_trip {
        actions.push(("flip_to_front".to_owned(), ZoomAction::Flip));
        actions.push(("flip_to_back".to_owned(), ZoomAction::Flip));
    }
    if config.wheel_drag.samples > 0 {
        actions.push(("tap_1_before_drag".to_owned(), ZoomAction::TapStop { ratio: 1.0 }));
        actions.push(("wheel_drag".to_owned(), ZoomAction::WheelDrag));
    }
    // Завершающий тап: экран остаётся на 1× основной камеры независимо от конфига.
    actions.push(("tap_1_final".to_owned(), ZoomAction::TapStop { ratio: 1.0 }));
    actions
        .into_iter()
        .enumerate()
        .map(|(index, (name, action))| ZoomStep {
            name: format!("{:02}_{name}", index + 1),
            action,
        })
        .collect()
}

/// Индекс кнопки ленты, ближайшей к коэффициенту `ratio`, и её роль. `None` — на шкале нет кнопок или ближайшая
/// дальше `MAX_STOP_DISTANCE` (запрошен коэффициент, которого на ленте нет: лучше провал плана, чем тап «рядом»).
pub fn nearest_stop(group: &LensGroup, ratio: f32) -> Option<(usize, ZoomStopKind)> {
    group
        .stops
        .iter()
        .enumerate()
        .map(|(index, stop)| (index, stop.kind, (stop.ratio.value() - ratio).abs()))
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .filter(|(_, _, distance)| *distance <= MAX_STOP_DISTANCE)
        .map(|(index, kind, _)| (index, kind))
}

/// Наибольшее расстояние между запрошенным коэффициентом и кнопкой ленты. Подпись `.6` — это 0,59 (решение 111).
pub const MAX_STOP_DISTANCE: f32 = 0.1;

/// Ожидание для тапа по кнопке `kind` при активной роли `current`: кнопка объектива зовёт эту роль, 2× (кроп)
/// остаётся на основной камере.
pub fn expectation_for_tap(kind: ZoomStopKind, current: Option<LensRole>) -> SwitchExpectation {
    let target = match kind {
        ZoomStopKind::Lens(role) => role,
        ZoomStopKind::DigitalCrop => LensRole::Wide,
    };
    if current == Some(target) {
        SwitchExpectation::NoSwitch
    } else {
        SwitchExpectation::ToRole(target)
    }
}

#[cfg(test)]
#[path = "zoom_plan_tests.rs"]
mod tests;
