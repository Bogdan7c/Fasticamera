//! Белая кадра со вспышкой: постоянный цвет светодиода активной камеры (WB11, WB41).
//!
//! Движок — владелец белой. Цвет светодиода не зависит от сцены, поэтому план кадра несёт готовую белую из
//! конфига (`three_a.flash.led_white`), выбранную по роли объектива, который снимает ЭТОТ снимок. Рендер
//! проявляет кадр с ней и больше ничего про белую вспышки не оценивает.

use camera_contract::control::{InvalidControlValue, WhiteBalanceSetting};
use camera_contract::ids::LensRole;

use super::super::CameraEngine;

impl CameraEngine {
    /// Роль объектива активной камеры (`None` — камеры нет в шкале зума: логическая и т. п.).
    fn active_lens_role(&self) -> Option<LensRole> {
        let camera = match &self.device {
            Some(device) => device.camera.clone(),
            None => self.desired_camera_id()?,
        };
        self.zoom_catalog_handle()
            .lens_for_camera(&camera)
            .map(|(_, lens)| lens.role)
    }

    /// Белая светодиода для плана кадра со вспышкой по роли активной камеры. `Err` бывает только при
    /// испорченном конфиге, который проверка при старте не пропускает; вызывающий отказывает в снимке.
    pub(super) fn led_white_for_active_camera(&self) -> Result<WhiteBalanceSetting, InvalidControlValue> {
        self.config
            .three_a
            .flash
            .led_white
            .white_for(self.active_lens_role())
            .to_setting()
    }
}
