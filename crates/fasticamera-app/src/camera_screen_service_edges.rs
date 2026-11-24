//! Края экрана на пути к экрану камеры: полосы системных жестов и безопасная зона.
//!
//! Оба факта приходят от платформы событиями-состояниями и нужны свежему экрану сразу при установке, но действуют
//! по-разному: полосы жестов — фильтр касаний (мимо редьюсера), безопасная зона — вход редьюсера, от неё зависит
//! раскладка раздела калибровки. Нет экрана — факт не потерян (он запомнен), поэтому в «отброшенный вход» не пишется.

use camera_screen::action::ScreenAction;
use shell_contract::safe_area::SafeAreaInsets;
use shell_contract::system_gesture::SystemGestureZones;

use super::CameraScreenService;
use crate::camera_screen_input::InputDelivery;

impl CameraScreenService {
    /// Зоны обязательных системных жестов: запоминает (для следующей установки экрана) и отдаёт установленному.
    pub fn route_system_gesture_zones(&mut self, zones: SystemGestureZones) {
        self.system_gesture_zones = zones;
        self.send_system_gesture_zones(zones);
    }

    /// Последние зоны системных жестов, известные сервису (для тестов и диагностики).
    pub fn system_gesture_zones(&self) -> SystemGestureZones {
        self.system_gesture_zones
    }

    /// Отдаёт свежему экрану известные края: сам он узнал бы о них только при смене на стороне платформы. «Ничего не
    /// сообщали» свежий экран и так считает своим началом, поэтому пустое не шлём.
    pub(super) fn announce_screen_edges(&mut self, safe_area: SafeAreaInsets) {
        if safe_area != SafeAreaInsets::NONE {
            self.route_safe_area(safe_area);
        }
        if !self.system_gesture_zones.is_none() {
            self.send_system_gesture_zones(self.system_gesture_zones);
        }
    }

    /// Безопасная зона — установленному экрану. Помнит её маршрутизатор (`DisplayState`, как ориентацию): экран,
    /// поставленный позже, получит её при установке.
    pub fn route_safe_area(&mut self, insets: SafeAreaInsets) {
        if self.installed.is_some() {
            self.send_action(ScreenAction::SafeAreaChanged(insets));
        }
    }

    fn send_system_gesture_zones(&mut self, zones: SystemGestureZones) {
        if let Some(installed) = &self.installed
            && installed.sender.send_system_gesture_zones(zones) == InputDelivery::ReceiverGone
        {
            self.dropped.actions += 1;
        }
    }
}
