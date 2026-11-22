//! Разовый запрос со вспышкой (сессия 14d, решение 158): проверка условий и построение плана.
//!
//! Разовая вспышка (`FLASH_MODE_SINGLE`) поверх повторяющегося запроса с `AE_MODE_OFF` освещает
//! кадр именно этого запроса (факт зонда 14d). Поэтому запрос обязан быть ручным (экспозицию
//! для кадра под вспышкой выбрал движок) и обязан идти тем же путём [`plan_request`], что и все
//! остальные: ключи частоты кадров и вендорские ключи нужны ему так же, как любому запросу
//! (ключи HAL «липкие», запрос без них сменил бы режим сенсора).
//!
//! Условия проверяются здесь, на ПК-тестируемой чистой функции, а не в слое NDK: бэкенд только
//! превращает отказ в `RequestFailure`.

use camera_contract::flash::FlashControl;
use camera_core::request_controls::ExposurePlan;

use crate::request_plan::{PlannedRequest, RequestInputs, plan_request};

/// Почему разовый запрос со вспышкой не строится.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FlashStillRefusal {
    /// В управлении не `Single`: вызов не по назначению (ошибка движка).
    #[error("снимок со вспышкой требует FlashControl::Single, получено {actual:?}")]
    NotSingleFlash { actual: FlashControl },
    /// Экспозицию ведёт вендорский AE: под вспышку его настроить нечем.
    #[error("снимок со вспышкой требует ручной экспозиции сенсора")]
    ExposureNotSensor,
    /// У активной камеры нет вспышки.
    #[error("у камеры нет вспышки")]
    NoFlashUnit,
}

/// Строит разовый запрос со вспышкой или отказывает с причиной. Состояния не меняет.
pub fn plan_flash_still(inputs: &RequestInputs<'_>) -> Result<PlannedRequest, FlashStillRefusal> {
    if inputs.controls.flash != FlashControl::Single {
        return Err(FlashStillRefusal::NotSingleFlash {
            actual: inputs.controls.flash,
        });
    }
    if !matches!(inputs.controls.exposure, ExposurePlan::Sensor { .. }) {
        return Err(FlashStillRefusal::ExposureNotSensor);
    }
    if !inputs.camera.flash.is_present() {
        return Err(FlashStillRefusal::NoFlashUnit);
    }
    // Триггера AF во вспышечном кадре нет: он освещает снимок, а не наводит фокус.
    Ok(plan_request(&RequestInputs {
        trigger: None,
        ..*inputs
    }))
}
