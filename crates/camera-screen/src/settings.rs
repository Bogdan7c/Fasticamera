//! Настройки съёмки, которыми владеет экран: вспышка, RAW, таймер, соотношение сторон, сетка, уровень.
//!
//! Это «желаемое пользователем», а не факты камеры: экран хранит выбор и передаёт его в снимок
//! (`ShotRequest`). Предустановленные значения (`#[default]`) — обычное состояние первого запуска;
//! сохранение между запусками («Preserve Settings») — сессия 16.

use camera_contract::viewfinder_crop::CropAspect;

/// Режим вспышки.
///
/// `Auto` существует как слот (у iOS: Auto → On → Off), но в v1 он **не поддержан** (инвариант 6,
/// `docs/ios-camera/feature-map.md`: решение «нужна ли вспышка» — вывод оценки AE, а не экрана).
/// Экран его никогда не выбирает: [`FlashMode::is_selectable`] возвращает `false`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FlashMode {
    /// Вспышка выключена.
    #[default]
    Off,
    /// Вспышка включена.
    On,
    /// Автовспышка. Не поддержана в v1 и не выбирается.
    Auto,
}

impl FlashMode {
    /// Можно ли выбрать режим сейчас (`Auto` — нет, см. тип).
    pub const fn is_selectable(self) -> bool {
        !matches!(self, FlashMode::Auto)
    }

    /// Следующий режим по тапу на значок над кадром: Off ↔ On (`Auto` пропускается).
    pub const fn toggled(self) -> FlashMode {
        match self {
            FlashMode::Off | FlashMode::Auto => FlashMode::On,
            FlashMode::On => FlashMode::Off,
        }
    }
}

/// Снимать ли RAW (DNG) рядом с основным файлом.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RawCapture {
    #[default]
    Off,
    On,
}

impl RawCapture {
    /// Противоположное значение (тап по значку RAW).
    pub const fn toggled(self) -> RawCapture {
        match self {
            RawCapture::Off => RawCapture::On,
            RawCapture::On => RawCapture::Off,
        }
    }
}

/// Таймер затвора. Имена вариантов — «слоты» iOS (3, 5, 10 с); сколько секунд в каждом слоте,
/// решает конфиг ([`crate::config::TimerConfig`]), чтобы длительность не была «магическим» числом.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TimerSetting {
    /// Таймер выключен: снимок сразу.
    #[default]
    Off,
    /// Короткий слот (по умолчанию 3 с).
    Seconds3,
    /// Средний слот (по умолчанию 5 с).
    Seconds5,
    /// Длинный слот (по умолчанию 10 с).
    Seconds10,
}

impl TimerSetting {
    /// Включён ли таймер (значок таймера над кадром виден только тогда, `behavior.md` 2.2).
    pub const fn is_active(self) -> bool {
        !matches!(self, TimerSetting::Off)
    }
}

/// Соотношение сторон кадра (маска видоискателя и кадрирование снимка).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AspectRatio {
    /// 4:3 — родной кадр сенсора, режим по умолчанию.
    #[default]
    FourByThree,
    /// 16:9 — кадрирование по высоте.
    SixteenByNine,
    /// 1:1 — квадрат по короткой стороне.
    Square,
}

impl AspectRatio {
    /// Все соотношения в порядке меню.
    pub const ALL: [AspectRatio; 3] = [
        AspectRatio::FourByThree,
        AspectRatio::SixteenByNine,
        AspectRatio::Square,
    ];

    /// То же соотношение на языке кадрирования сенсора (`camera-contract`).
    pub const fn crop_aspect(self) -> CropAspect {
        match self {
            AspectRatio::FourByThree => CropAspect::FOUR_BY_THREE,
            AspectRatio::SixteenByNine => CropAspect::SIXTEEN_BY_NINE,
            AspectRatio::Square => CropAspect::SQUARE,
        }
    }
}

/// Сетка поверх кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GridSetting {
    #[default]
    Off,
    On,
}

/// Уровень (линия горизонта по наклону телефона).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LevelSetting {
    #[default]
    Off,
    On,
}

/// Все настройки съёмки одним значением.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CaptureSettings {
    pub flash: FlashMode,
    pub raw: RawCapture,
    pub timer: TimerSetting,
    pub aspect: AspectRatio,
    pub grid: GridSetting,
    pub level: LevelSetting,
}
