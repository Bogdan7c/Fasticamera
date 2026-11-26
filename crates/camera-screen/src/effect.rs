//! Намерения экрана наружу. Экран ничего не вызывает — эффекты исполняет приложение
//! (`fasticamera-app`): камеру, сервис снимка, хаптику, галерею и рендер.

use camera_contract::control::{ExposureBias, WhiteBalanceSetting};
use camera_contract::three_a::SensorPoint;
use camera_contract::white::{WhiteAdjustment, WhiteSourceId};
use camera_contract::zoom::LensIntent;
use look_contract::{CleanView, LookIntent};
use scope_contract::ScopeIntent;
use shell_contract::haptics::HapticPattern;
use shell_contract::text_input::TextPurpose;

use crate::layout::FramingIntent;
use crate::mode_behaviour::WhiteTrackingHold;
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
    /// `set_white`: белое целиком рукой (кнопка света). Переход всегда мгновенный (`WhiteTransition::Immediate`).
    SetWhite(WhiteBalanceSetting),
    /// `adjust_white`: одна ось белого рукой (шкала); вторую ось держит движок.
    AdjustWhite(WhiteAdjustment),
    /// `take_white`: взять белое у источника (кнопка «Телефон», долгое нажатие по значку).
    TakeWhite(WhiteSourceId),
    /// `follow_white`: следить за белым источника (кнопка «Auto», WB44). Включённость слежения экран берёт из
    /// фактов; любое другое действие человека с белым выключает его на стороне движка.
    FollowWhite(WhiteSourceId),
    /// `set_white_follow_pause`: пауза слежения за белым на время раздела калибровки (решение 289). Экран эту
    /// команду НЕ выдаёт эффектом: он отвечает на запрос `ScreenState::white_tracking_hold()`, а приложение каждый
    /// кадр сверяет желание с принятым движком и строит это намерение само, пока разница не принята (уровень, а не
    /// событие). Вариант здесь лишь для того, чтобы порт камеры нёс его тем же словарём, что остальные намерения.
    HoldWhiteTracking(WhiteTrackingHold),
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
    /// Намерение по виду картинки и пресетам (решение 178). Исполнитель эффектов на потоке рендера отдаёт его
    /// библиотеке пресетов, шлёт новый вид рендеру и просит записать файл; экран к ним не прикасается.
    Look(LookIntent),
    /// Показать или скрыть цветовой прибор (решения 183, 188–190). Исполнитель шлёт его рендеру командой
    /// видоискателя; экран приборы не считает и не рисует, а запрос держит, пока открыт его раздел (виджет — 14b).
    Scopes(ScopeIntent),
    /// «Чистяк»: показать основу без вида или вернуть правки (решение 189). Идемпотентно: экран шлёт только
    /// смену, повторных `ShowBase` без `ShowEdited` между ними не бывает.
    CleanView(CleanView),
    /// Попросить у оболочки короткий текст (имя пресета, решение 197). Номер запроса выдаёт приложение (брокер
    /// ввода) и возвращает экрану действием `TextRequestIssued`; ответ придёт действием `TextInputFinished`.
    /// Если запрос не удалось отправить — действие `TextRequestNotSent`.
    RequestText {
        /// Для чего текст (платформа выбирает заголовок диалога).
        purpose: TextPurpose,
        /// Что подставить в поле при показе (пусто — пустое поле).
        initial_text: String,
    },
}
