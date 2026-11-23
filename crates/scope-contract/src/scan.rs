//! Как глаз читает показанную картинку слева направо: ось «столбцов» waveform и parade.

use serde::{Deserialize, Serialize};

/// В какую сторону показанной картинки (как она лежит на портретной поверхности экрана) идёт взгляд
/// «слева направо» по СЦЕНЕ.
///
/// Раскладка экрана всегда портретная, а камера смотрит в сцену: если телефон держат боком, левоправая
/// ось сцены ложится на вертикаль поверхности. Waveform и parade обязаны показывать сцену так, как её видит
/// глаз, поэтому горизонталь следа берётся вдоль этого направления, а не вдоль столбцов поверхности.
///
/// Соответствие положению телефона (`shell_contract::orientation::DeviceOrientation`; те же соглашения,
/// что у `icon_rotation_clockwise_degrees` в `camera-screen`; экран заполняет поле из ориентации):
///
/// | положение телефона | угол значков по часовой | вариант |
/// |---|---|---|
/// | `Portrait` | 0° | [`ScopeScan::Upright`] |
/// | `LandscapeLeft` (верх телефона смотрит влево) | +90° | [`ScopeScan::RotatedClockwise`] |
/// | `PortraitUpsideDown` | 180° | [`ScopeScan::UpsideDown`] |
/// | `LandscapeRight` (верх телефона смотрит вправо) | −90° | [`ScopeScan::RotatedCounterClockwise`] |
///
/// Порядок композиции с зеркалом фронтальной камеры: зеркало уже применено к показанной картинке
/// (она зеркальна, как на экране), а `ScopeScan` описывает, как глаз читает ИМЕННО её. Поворот не отменяет
/// зеркало и не зависит от него.
///
/// Вектороскоп от поля не зависит: он меряет цвет, а не положение.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeScan {
    /// Слева направо по столбцам поверхности (портрет): `x` растёт — взгляд идёт вправо.
    Upright,
    /// Сверху вниз по строкам поверхности (телефон боком, `LandscapeLeft`).
    RotatedClockwise,
    /// Справа налево по столбцам поверхности (телефон вверх ногами).
    UpsideDown,
    /// Снизу вверх по строкам поверхности (телефон боком, `LandscapeRight`).
    RotatedCounterClockwise,
}

impl ScopeScan {
    /// Все четыре чтения (для тестов и перебора).
    pub const ALL: [ScopeScan; 4] = [
        ScopeScan::Upright,
        ScopeScan::RotatedClockwise,
        ScopeScan::UpsideDown,
        ScopeScan::RotatedCounterClockwise,
    ];

    /// Идёт ли взгляд вдоль строк поверхности (вертикаль поверхности — ось сцены «слева направо»).
    pub const fn runs_along_surface_rows(self) -> bool {
        matches!(self, Self::RotatedClockwise | Self::RotatedCounterClockwise)
    }

    /// Взгляд идёт против возрастания координаты вдоль своей оси поверхности.
    pub const fn is_reversed(self) -> bool {
        matches!(self, Self::UpsideDown | Self::RotatedCounterClockwise)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_and_direction_of_every_scan_are_distinct() {
        let described: Vec<(bool, bool)> = ScopeScan::ALL
            .iter()
            .map(|scan| (scan.runs_along_surface_rows(), scan.is_reversed()))
            .collect();
        assert_eq!(described, [(false, false), (true, false), (false, true), (true, true)]);
    }

    #[test]
    fn serde_names_are_snake_case_and_round_trip() {
        for scan in ScopeScan::ALL {
            let text = serde_json::to_string(&scan).unwrap();
            assert_eq!(serde_json::from_str::<ScopeScan>(&text).unwrap(), scan);
        }
        assert_eq!(
            serde_json::to_string(&ScopeScan::RotatedCounterClockwise).unwrap(),
            r#""rotated_counter_clockwise""#
        );
    }
}
