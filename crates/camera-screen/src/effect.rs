//! Намерения экрана наружу. Экран ничего не вызывает — эффекты исполняет приложение
//! (`fasticamera-app`): камеру, сервис снимка, хаптику, галерею и рендер.

use camera_contract::control::ExposureBias;
use camera_contract::three_a::SensorPoint;
use camera_contract::zoom::LensIntent;
use shell_contract::haptics::HapticPattern;

use crate::layout::FramingIntent;
use crate::shutter::ShotRequest;

/// Намерение камере. Имена совпадают с методами `CameraHandle` (`camera-core/src/handle.rs`),
/// чтобы проводка в приложении была прямой.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScreenCameraIntent {
    /// `focus_and_expose_at`: тап. Снимает блокировку AE/AF сам (док `CameraCommand::FocusAndExposeAt`),
    /// поэтому отдельный сброс экрану не нужен.
    FocusAndExposeAt(SensorPoint),
    /// `lock_focus_and_exposure_at`: долгий тап.
    LockFocusAndExposureAt(SensorPoint),
    /// `reset_point_of_interest`: вернуть фокус и замер на весь кадр, сняв блокировку.
    ResetPointOfInterest,
    /// `set_exposure_bias`.
    SetExposureBias(ExposureBias),
}

/// Намерение наружу. С сессии 13 не `Copy`: кадрирование несёт общий каталог зума (`Arc`).
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Команда камере.
    Camera(ScreenCameraIntent),
    /// Запрос снимка сервису съёмки.
    Capture(ShotRequest),
    /// Звук затвора: выдаётся в момент снимка вместе с `Capture` (при таймере — в момент срабатывания),
    /// а не при нажатии кнопки таймера. Исполняет приложение; отказ платформы — счётчик, не ошибка экрана.
    PlayShutterSound,
    /// Хаптика по таблице `haptics.md` (тики шкал играют сами элементы, не редьюсер).
    Haptic(HapticPattern),
    /// Открыть системную галерею.
    OpenSystemGallery,
    /// Где и какую часть сенсора рисовать видоискателем (решение 93). Рендер получает это
    /// через приложение.
    Viewfinder(FramingIntent),
    /// Какой объектив показывать (сессия 13): направление + коэффициент. Приложение кладёт намерение в ящик
    /// «последнее побеждает» (`LensIntentChannel`), а НЕ в очередь команд камеры: промежуточные значения колеса
    /// и щипка не должны копиться. Экран не выбирает камеру — это делает движок с гистерезисом.
    Lens(LensIntent),
}
