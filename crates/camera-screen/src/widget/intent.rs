//! Намерения жестов экрана: тип `I` арены `UiScene<I>`. Каждый распознаватель экрана несёт одно из них,
//! а фасад по нему передаёт событие нужному виджету.

use look_contract::{WheelPart, WheelZone};
use scope_contract::{CieDiagram, HistogramMode, ScopeKind, VectorscopeAxes, VectorscopeZoom};

use crate::calibration::ControlViewKind;
use crate::white::WhiteAxis;
use crate::widget::white_card_site::WhiteCardButton;

/// Кнопка экрана (нажатие = `Press` для подсветки/срабатывания + `Tap` для арбитража, дока `UiScene`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ButtonId {
    Shutter,
    Thumbnail,
    MenuButton,
    /// Значок над кадром по номеру слота ряда.
    StripSlot(u8),
    /// Пункт сетки меню по номеру слота.
    MenuItem(u8),
    /// Вариант в ряду нижней карточки по номеру слота.
    CardOption(u8),
    /// Левая кнопка карточки: «‹» назад в меню или «×» закрыть.
    CardLeft,
    /// Правая кнопка карточки: «↻» сброс (только Exposure).
    CardRight,
    /// Кнопка ленты зума по номеру упора в каталоге.
    ZoomStop(u8),
    /// Кнопка переворота камеры.
    FlipCamera,
    /// «↻» у органа управления раздела калибровки по номеру строки: 0–7 — глобальные ручки, 8–10 — оси полосы,
    /// 11 — основа тона, 12 — кольцо яркости выбранного колеса.
    CalibrationReset(u8),
    /// Сегмент переключателя вида колонки SCALES | WHEELS (решение 204).
    CalibrationControlView(ControlViewKind),
    /// Вкладка колеса LIFT · GAMMA · GAIN · OFFSET (решение 205).
    CalibrationWheelTab(WheelZone),
    /// «↻» части выбранного колеса, кроме кольца (кольцо сбрасывает «↻» своей шкалы): `Whole` — у названия колеса,
    /// `Push` — в углу круга (решение 207).
    CalibrationWheelReset(WheelPart),
    /// Цветная точка полосы оттенка по номеру в ряду.
    CalibrationBandDot(u8),
    /// «Чистяк»: пока палец на кнопке, на превью и приборе основа без вида.
    CleanView,
    /// Кнопка меню «шесть точек» внутри раздела калибровки.
    CalibrationMenu,
    /// Таблетка с именем пресета в карточке Styles экрана PHOTO по номеру в ряду.
    StyleChip(u8),
    /// Точка страницы в полоске прибора: тап выбирает этот прибор (решение 258).
    ScopePageDot(ScopeKind),
    /// Сегмент переключателя осей вектороскопа в полоске прибора (Y'CbCr | OKLab, решение 258).
    ScopeAxes(VectorscopeAxes),
    /// Кнопка «2×» в полоске вектороскопа: увеличение центра вдвое (решение 325).
    ScopeZoom(VectorscopeZoom),
    /// Сегмент режима гистограммы в полоске прибора: RGB | PARADE | Y (решение 331).
    ScopeHistogramMode(HistogramMode),
    /// Сегмент шкалы CIE в полоске прибора: xy | u'v' (решение 332).
    ScopeCieDiagram(CieDiagram),
    /// Кнопка белого рядом с затвором: у неё один собственный узел (тап открывает карточку, долгое нажатие берёт
    /// белое телефона).
    WhiteButton,
    /// «×» карточки белого.
    WhiteClose,
    /// Число оси на карточке белого (тап отдаёт шкалу этой оси).
    WhiteAxis(WhiteAxis),
    /// Кнопка «Auto» на карточке белого (слежение за белым телефона).
    WhiteAuto,
    /// Кнопка «Phone» на карточке белого.
    WhitePhone,
    /// Кнопка света на карточке белого по номеру в конфиге.
    WhiteLight(u8),
    /// Капсула белого в правой части полоски прибора раздела калибровки (решение 290).
    SectionWhiteCapsule,
    /// Кнопка карточки белого В РАЗДЕЛЕ калибровки (решение 292). Отдельные номера от PHOTO нужны, потому что карточка
    /// белого PHOTO живёт всегда, и событие с общим номером ушло бы не в тот виджет.
    SectionWhite(WhiteCardButton),
    /// Левая кнопка карточки меню раздела: «‹» закрыть меню.
    CalibrationCardLeft,
    /// Правая кнопка карточки меню раздела: «↻» сбросить правки пресета.
    CalibrationCardRight,
    /// Таблетка верхнего ряда карточки меню раздела (имена пресетов) по номеру в ряду.
    CalibrationChip(u8),
    /// Таблетка ряда действий карточки пресетов (NEW / RENAME / SAVE / DELETE) по номеру в ряду.
    CalibrationAction(u8),
}

/// Намерение жеста.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScreenGesture {
    /// `Press` кнопки: подсветка и срабатывание по отпусканию.
    Press(ButtonId),
    /// `Tap` кнопки: нужен только арбитражу (отнимает касание у тапа видоискателя).
    ButtonTapGuard(ButtonId),
    /// Тап по фону/видоискателю: фокус или закрытие панели.
    ViewfinderTap,
    /// Долгий тап по видоискателю: AE/AF Lock.
    ViewfinderLongPress,
    /// Горизонтальный пан (по видоискателю или капсуле): карусель режимов.
    ModesPan,
    /// Пан вверх (по видоискателю или капсуле): открыть меню Controls.
    MenuOpenPan,
    /// Вертикальный пан «солнца» рядом с рамкой фокуса.
    SunPan,
    /// Тап по капсуле режимов: выбрать режим под пальцем (или открыть меню на текущем).
    CapsuleTap,
    /// Пан вниз по шторке меню: закрыть.
    DrawerClosePan,
    /// Горизонтальный пан линейки Exposure в карточке.
    ExposureSliderPan,
    /// Долгое нажатие на кнопку белого (рецепт кнопки с долгим нажатием: `Press` + `Tap` + `LongPress`).
    WhiteButtonLongPress,
    /// Горизонтальный пан шкалы на карточке белого (ведёт шкалу выбранной оси).
    WhiteRulerPan,
    /// Щипок по видоискателю: зум. Побеждает пан и тапы видоискателя вторым пальцем (`ExtraFingers::FailOrCancel`).
    ViewfinderPinch,
    /// Долгий тап по ленте зума: открывает колесо, палец ведёт его.
    ZoomStripLongPress,
    /// Горизонтальный пан по ленте зума: открывает колесо, палец ведёт его.
    ZoomStripPan,
    /// Горизонтальный пан по открытому колесу.
    ZoomWheelPan,
    /// Нажатие на открытое колесо (палец лёг / поднят): держит колесо открытым.
    ZoomWheelPress,
    /// Пан вдоль линейки ручки раздела калибровки (номер строки как у `ButtonId::CalibrationReset`).
    CalibrationRulerPan(u8),
    /// Пан по кругу толчка выбранного колеса: палец в любом месте круга тянет точку (решение 206).
    CalibrationWheelPadPan,
    /// Пан по списку ручек (прокрутка в осях «глаза»).
    CalibrationListPan,
    /// Свайп по переключателю основы тона.
    CalibrationBasePan,
    /// Тап по переключателю основы тона: выбирает видимую соседнюю модель (решение 229).
    CalibrationBaseTap,
    /// Свайп по зоне прибора: смена прибора.
    CalibrationScopePan,
    /// Свайп по капсуле режимов внутри раздела калибровки.
    CalibrationModesPan,
    /// Тап по капсуле режимов внутри раздела калибровки.
    CalibrationCapsuleTap,
    /// Пан ряда имён пресетов в карточке Styles экрана PHOTO.
    StyleChipsPan,
    /// Пан верхнего ряда таблеток карточки меню раздела калибровки.
    CalibrationChipsPan,
    /// Тап мимо меню раздела: закрывает его (аналог решения 94).
    CalibrationMenuScrimTap,
    /// Пан по шторке или карточке меню раздела: вниз глазами — закрыть.
    CalibrationMenuClosePan,
    /// Пан шкалы карточки белого в разделе калибровки (в глазных осях; свой номер, как у кнопок карточки).
    SectionWhiteRulerPan,
}

impl ScreenGesture {
    /// Принадлежит ли жест разделу калибровки (его виджеты живут, только пока раздел показан; жесты, пришедшие
    /// после ухода раздела, — это отмена, и экран гасит их сам).
    pub fn belongs_to_calibration(self) -> bool {
        match self {
            ScreenGesture::CalibrationRulerPan(_)
            | ScreenGesture::CalibrationWheelPadPan
            | ScreenGesture::CalibrationListPan
            | ScreenGesture::CalibrationBasePan
            | ScreenGesture::CalibrationBaseTap
            | ScreenGesture::CalibrationScopePan
            | ScreenGesture::CalibrationModesPan
            | ScreenGesture::CalibrationCapsuleTap
            | ScreenGesture::CalibrationChipsPan
            | ScreenGesture::CalibrationMenuScrimTap
            | ScreenGesture::CalibrationMenuClosePan
            | ScreenGesture::SectionWhiteRulerPan => true,
            ScreenGesture::Press(button) | ScreenGesture::ButtonTapGuard(button) => matches!(
                button,
                ButtonId::CalibrationReset(_)
                    | ButtonId::CalibrationControlView(_)
                    | ButtonId::CalibrationWheelTab(_)
                    | ButtonId::CalibrationWheelReset(_)
                    | ButtonId::CalibrationBandDot(_)
                    | ButtonId::CleanView
                    | ButtonId::CalibrationMenu
                    | ButtonId::ScopePageDot(_)
                    | ButtonId::ScopeAxes(_)
                    | ButtonId::ScopeZoom(_)
                    | ButtonId::ScopeHistogramMode(_)
                    | ButtonId::ScopeCieDiagram(_)
                    | ButtonId::SectionWhiteCapsule
                    | ButtonId::SectionWhite(_)
                    | ButtonId::CalibrationCardLeft
                    | ButtonId::CalibrationCardRight
                    | ButtonId::CalibrationChip(_)
                    | ButtonId::CalibrationAction(_)
            ),
            _ => false,
        }
    }
}
