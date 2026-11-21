//! Публичное состояние смены объектива (сессия 13): что снимок и события говорят снаружи.
//!
//! Владелец состояния — движок (`engine::lens_switch`); здесь только словарь, который читают
//! экран камеры (удержать размытый кадр, проявить новую камеру), приложение и smoke.

use std::sync::Arc;
use std::time::Duration;

use camera_contract::availability::CameraUnavailableReason;
use camera_contract::ids::{CameraId, LensFacing, LensRole};
use camera_contract::zoom::{LensIntent, ZoomCatalog};
use serde::Serialize;

use crate::backend::ConfigureFailure;
use crate::standby_status::StandbyStatus;

/// Где движок в смене объектива.
///
/// * `Idle` — кадров ещё нет (камера не открыта, поток остановлен): показывать нечего,
///   первый кадр любой камеры — не «переключение», а первый показ;
/// * `Steady` — кадры идут от камеры роли `role`;
/// * `Switching` — желаемая камера роли `to`, последним показанным был объектив `from`; пока
///   первый кадр `to` не пришёл, экран держит размытый последний кадр `from` (решение 110).
///   `from == to` значит «возврат на прежний объектив» (смену отменили или она не удалась).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum LensSwitchState {
    Idle,
    Steady { role: LensRole },
    Switching { from: LensRole, to: LensRole },
}

/// Каким путём исполнена смена объектива (по первому кадру новой основной).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LensSwitchMethod {
    /// Старая камера закрыта, новая открыта и сконфигурирована заново.
    #[default]
    Reopened,
    /// Новая камера уже стримила «тёплой» и повышена обменом ролей без закрытия и открытия.
    PromotedStandby,
}

/// Почему смена на новую камеру не удалась (вина именно новой камеры).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum LensSwitchFailure {
    /// Новая камера не открылась или отказала до первого кадра (сбой устройства или службы,
    /// камеры нет). Вытеснение, «занята», лимит камер, разрешение и политика сюда не попадают:
    /// это не вина камеры, их лечит обычное восстановление (решение 39).
    Unavailable { reason: CameraUnavailableReason },
    /// Новая камера не приняла режим потока даже после отката частоты.
    StreamRejected { failure: ConfigureFailure },
}

/// Роль, которую селектор временно не выбирает, потому что смена на неё провалилась.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LensBlock {
    pub role: LensRole,
    pub reason: LensSwitchFailure,
}

/// Сконфигурированная камера, которая сейчас отдаёт поток.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActiveLens {
    pub role: LensRole,
    pub camera: CameraId,
    pub facing: LensFacing,
}

/// Счётчики смен объектива за жизнь процесса.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct LensCounters {
    /// Завершённые смены на **другой** объектив (возврат на прежний не считается).
    pub switches_completed: u64,
    pub switches_failed: u64,
    /// Промежуточные цели, отменённые новой целью посреди смены.
    pub switches_retargeted: u64,
    /// Намерения экрана, которые движок не смог выполнить (нет камеры направления, формат не
    /// предлагается): ничего не сломано, но экран должен это видеть.
    pub intents_ignored: u64,
    /// От намерения/команды до первого кадра новой камеры, последняя завершённая смена.
    pub last_switch_elapsed: Option<Duration>,
}

/// Состояние объективов в снимке [`crate::CameraStatus`].
#[derive(Debug, Clone, PartialEq)]
pub struct LensStatus {
    /// Шкала зума; перестраивается при загрузке каталога камер.
    pub zoom_catalog: Arc<ZoomCatalog>,
    /// Камера, чей поток сконфигурирован; `None` — сессии нет.
    pub active: Option<ActiveLens>,
    pub switch: LensSwitchState,
    /// Последнее принятое намерение экрана.
    pub last_intent: Option<LensIntent>,
    /// Роль, на которую смена провалилась и которую селектор не выбирает, пока намерение не
    /// выберет другую роль или команда не укажет эту явно.
    pub blocked: Option<LensBlock>,
    pub counters: LensCounters,
    /// Тёплые камеры (13b).
    pub standby: StandbyStatus,
}

impl LensStatus {
    /// Начальное состояние до чтения каталога.
    pub fn initial() -> Self {
        Self {
            zoom_catalog: Arc::new(ZoomCatalog::default()),
            active: None,
            switch: LensSwitchState::Idle,
            last_intent: None,
            blocked: None,
            counters: LensCounters::default(),
            standby: StandbyStatus::default(),
        }
    }
}
