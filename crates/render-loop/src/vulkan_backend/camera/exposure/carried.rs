//! Ожидаемая экспозиция нового потока на стороне рендера (сессия 13b, решения 118 и 126).
//!
//! Движок публикует в указании `stream_start` («для камеры X ожидаем такое произведение») ДО смены потока.
//! Рендер применяет её к кадру только если выполнены оба условия:
//! 1. камера кадра совпадает с камерой оценки: оценка чужой камеры бессмысленна (другая чувствительность);
//! 2. у потока ещё нет собственной цели — это проверяет контроллер: первый принятый замер вытесняет ожидание.

use camera_contract::auto_exposure::{ExposureDirective, ExposureProduct};
use camera_contract::frame_passport::RawFramePassport;

/// Ожидаемое произведение «выдержка × ISO» для кадра `passport`; `None` — движок ничего не передал или передал
/// для другой камеры.
pub(super) fn expected_target(directive: &ExposureDirective, passport: &RawFramePassport) -> Option<ExposureProduct> {
    directive
        .stream_start
        .filter(|carried| carried.is_for(&passport.stream.camera))
        .map(|carried| carried.target)
}
