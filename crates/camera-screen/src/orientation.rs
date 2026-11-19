//! Ориентация значков. Раскладка экрана всегда портретная (`behavior.md` 2.7): при повороте
//! телефона на месте поворачиваются только значки. Здесь — целевой угол; пружину плавного
//! поворота ведут виджеты (P5b, `motion.md`).

use shell_contract::orientation::DeviceOrientation;

/// Целевой угол поворота значков в градусах **по часовой стрелке** (как видит пользователь).
///
/// Телефон повёрнут на 90° против часовой (`LandscapeLeft`, верх смотрит влево) — чтобы значки
/// остались «вертикальными» для глаза, их надо повернуть на 90° по часовой, и наоборот.
/// `PortraitUpsideDown` — ровно 180°; в какую сторону крутить (±180°), выбирает анимация по
/// кратчайшему пути от текущего угла.
pub fn icon_rotation_clockwise_degrees(orientation: DeviceOrientation) -> f32 {
    match orientation {
        DeviceOrientation::Portrait => 0.0,
        DeviceOrientation::LandscapeLeft => 90.0,
        DeviceOrientation::LandscapeRight => -90.0,
        DeviceOrientation::PortraitUpsideDown => 180.0,
    }
}
