//! Поведение режима как ДАННЫЕ (решения 94, 185): затвор, клавиша громкости, раскладка, место видоискателя и
//! наличие раздела калибровки задаёт запись режима в реестре, а не `if mode == …` в коде затвора или раскладки.
//!
//! Новый режим — новая запись [`ModeBehaviour`] в `ModeRegistry`; код затвора, меню и раскладки при этом не
//! правится: он спрашивает поведение у текущего режима (`ScreenState::mode_behaviour`).

/// Есть ли у режима затвор (кнопка на экране и сам снимок).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutterPresence {
    /// Затвор есть: кнопка рисуется и снимает.
    Present,
    /// Затвора нет (раздел калибровки, решение 185: «из раздела не снимают»): нажатие отвергается.
    Absent,
}

/// Есть ли у режима управление камерой: фокус по тапу, AE/AF LOCK, компенсация экспозиции, зум и значки над кадром.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraControls {
    /// Управление есть (PHOTO).
    Present,
    /// Управления нет (раздел калибровки): такие действия редьюсер отвергает сам, а не полагается на то, что
    /// виджетов для них на экране нет.
    Absent,
}

/// Что делает аппаратная клавиша громкости в режиме.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeKeyRole {
    /// Снимает (решение 94: и при открытом меню Controls).
    Shoots,
    /// Игнорируется: в разделе калибровки клавиша громкости не снимает (исключение из решения 94).
    Ignored,
}

/// Как раскладка режима относится к повороту телефона.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutPolicy {
    /// Раскладка всегда портретная, при повороте крутятся только значки (решение 165).
    PortraitFixed,
    /// Раскладка перестраивается под положение телефона (раздел калибровки, решение 185: «обе ориентации»).
    FollowsOrientation,
}

/// Где режим держит видоискатель.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewfinderPlacement {
    /// Кадр выбранного соотношения сторон на привычном месте (PHOTO).
    FramedByAspect,
    /// Один видоискатель на весь экран, режим «cover» (решение 203): стекло ручек и прибора лежит поверх.
    FullScreenCover,
}

/// Есть ли у режима раздел калибровки: ручки вида, приборы, меню пресетов, кнопка «чистяк».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationSurface {
    /// Раздела нет: меню «шесть точек» открывает меню Controls, приборы скрыты.
    Absent,
    /// Раздел есть: меню «шесть точек» открывает меню калибровки, прибор показан, «чистяк» работает.
    Present,
}

/// Поведение режима. Все поля — данные: проверяются тестами реестра, а не логикой обработчиков.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModeBehaviour {
    /// Есть ли затвор.
    pub shutter: ShutterPresence,
    /// Роль клавиши громкости.
    pub volume_key: VolumeKeyRole,
    /// Как раскладка следует за поворотом.
    pub layout: LayoutPolicy,
    /// Где видоискатель.
    pub viewfinder: ViewfinderPlacement,
    /// Есть ли раздел калибровки.
    pub calibration: CalibrationSurface,
    /// Есть ли управление камерой (фокус, экспозиция, зум, значки над кадром).
    pub camera_controls: CameraControls,
}

impl ModeBehaviour {
    /// PHOTO: затвор есть, громкость снимает, раскладка портретная, кадр по соотношению сторон.
    pub const PHOTO: ModeBehaviour = ModeBehaviour {
        shutter: ShutterPresence::Present,
        volume_key: VolumeKeyRole::Shoots,
        layout: LayoutPolicy::PortraitFixed,
        viewfinder: ViewfinderPlacement::FramedByAspect,
        calibration: CalibrationSurface::Absent,
        camera_controls: CameraControls::Present,
    };

    /// CALIBRATE (решения 185, 203): затвора нет, громкость не снимает, обе ориентации, видоискатель на весь экран.
    pub const CALIBRATE: ModeBehaviour = ModeBehaviour {
        shutter: ShutterPresence::Absent,
        volume_key: VolumeKeyRole::Ignored,
        layout: LayoutPolicy::FollowsOrientation,
        viewfinder: ViewfinderPlacement::FullScreenCover,
        calibration: CalibrationSurface::Present,
        camera_controls: CameraControls::Absent,
    };

    /// Есть ли у режима раздел калибровки.
    pub const fn has_calibration_surface(self) -> bool {
        matches!(self.calibration, CalibrationSurface::Present)
    }

    /// Есть ли у режима управление камерой.
    pub const fn has_camera_controls(self) -> bool {
        matches!(self.camera_controls, CameraControls::Present)
    }

    /// Есть ли у режима затвор.
    pub const fn has_shutter(self) -> bool {
        matches!(self.shutter, ShutterPresence::Present)
    }
}

/// Доступность режима калибровки в реестре (флаг конфига, решение 185).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationAvailability {
    /// Режим CALIBRATE стоит в карусели рядом с PHOTO.
    Enabled,
    /// Режима нет: приложение выглядит как до 14b.
    Disabled,
}
