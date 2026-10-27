//! Нейтральные параметры предпросмотра RAW-мозаики и их проверка.
//!
//! `gpu-vulkan` не знает про `camera-contract`: вызывающий (render-loop) переводит паспорт кадра
//! в эти типы. Все допущения шейдера (чётные границы полезной области, известная раскладка CFA)
//! проверяются здесь ДО записи команд — шейдер не получает параметры, с которыми он гадает.

use ash::vk;

use crate::error::GpuError;
use crate::hardware_buffer_cache::queue_handoff::{BufferAcquire, ImageAcquire, ReleaseTarget};

/// Раскладка цветного фильтра квадрата 2×2 в порядке строк. Прочие (RGB/моно/NIR) предпросмотр
/// не умеет: явная ошибка вместо догадки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CfaOrder {
    /// R G / G B.
    Rggb,
    /// G R / B G.
    Grbg,
    /// G B / R G.
    Gbrg,
    /// B G / G R (OnePlus 11).
    Bggr,
}

/// Цветовой канал позиции в ячейке CFA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CfaChannel {
    /// Красный.
    Red,
    /// Зелёный (в ячейке их два).
    Green,
    /// Синий.
    Blue,
}

impl CfaOrder {
    /// Каналы позиций ячейки в порядке строк: (0,0), (1,0), (0,1), (1,1).
    pub const fn channels(self) -> [CfaChannel; 4] {
        use CfaChannel::{Blue, Green, Red};
        match self {
            Self::Rggb => [Red, Green, Green, Blue],
            Self::Grbg => [Green, Red, Blue, Green],
            Self::Gbrg => [Green, Blue, Red, Green],
            Self::Bggr => [Blue, Green, Green, Red],
        }
    }

    /// Код раскладки для шейдера (таблица `CHANNELS` в `raw_mosaic_params.glsl`).
    pub(crate) const fn shader_code(self) -> u32 {
        match self {
            Self::Rggb => 0,
            Self::Grbg => 1,
            Self::Gbrg => 2,
            Self::Bggr => 3,
        }
    }
}

/// Поворот по часовой стрелке в четвертях оборота. Явный тип вместо числа 0..3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuarterTurns {
    /// Без поворота.
    None,
    /// 90° по часовой.
    Clockwise90,
    /// 180°.
    Clockwise180,
    /// 270° по часовой.
    Clockwise270,
}

impl QuarterTurns {
    /// Из числа четвертей оборота; `None` — число вне 0..=3.
    pub const fn from_count(count: u32) -> Option<Self> {
        match count {
            0 => Some(Self::None),
            1 => Some(Self::Clockwise90),
            2 => Some(Self::Clockwise180),
            3 => Some(Self::Clockwise270),
            _ => Option::None,
        }
    }

    /// Число четвертей оборота (0..=3).
    pub const fn count(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Clockwise90 => 1,
            Self::Clockwise180 => 2,
            Self::Clockwise270 => 3,
        }
    }

    /// Меняет ли поворот местами ширину и высоту.
    pub const fn swaps_axes(self) -> bool {
        self.count() % 2 == 1
    }
}

/// Полезная область буфера в пикселях сенсора.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidArea {
    /// Левый край (чётный).
    pub left: u32,
    /// Верхний край (чётный).
    pub top: u32,
    /// Ширина (чётная, ≥ 2).
    pub width: u32,
    /// Высота (чётная, ≥ 2).
    pub height: u32,
}

/// Параметры предпросмотра одного кадра. Всё, что зависит от кадра и не лежит в самом буфере.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MosaicPreviewParams {
    /// Где в буфере настоящие данные (остальное — нули/мусор и не показывается).
    pub valid_area: ValidArea,
    /// Раскладка цветного фильтра.
    pub cfa: CfaOrder,
    /// Чёрный уровень по позициям ячейки 2×2 в порядке строк.
    pub black_levels: [u32; 4],
    /// Уровень насыщения.
    pub white_level: u32,
    /// На сколько четвертей по часовой повернуть кадр сенсора, чтобы он встал прямо.
    pub sensor_rotation: QuarterTurns,
}

/// Откуда шейдер читает кадр и как ресурс передаётся между камерой и нашей очередью.
///
/// Дескрипторы — «голые» хэндлы: владелец ресурса (импорт кадра) обязан пережить сабмит, который
/// их читает; следит за этим вызывающий (render-loop держит импорт до конца работы GPU).
#[derive(Debug, Clone, Copy)]
pub enum MosaicSource {
    /// RAW16: картинка `R16_UINT` (sampled, `texelFetch`).
    SampledR16 {
        /// Представление с usage `SAMPLED`.
        view: vk::ImageView,
        /// Сама картинка (для барьеров).
        image: vk::Image,
        /// Ширина картинки, пиксели.
        width: u32,
        /// Высота картинки, пиксели.
        height: u32,
        /// Как забрать картинку перед чтением.
        acquire: ImageAcquire,
        /// Куда отдать после чтения.
        release: ReleaseTarget,
    },
    /// RAW10: упакованный буфер (`STORAGE_BUFFER`), 4 пикселя в 5 байтах.
    PackedRaw10 {
        /// Буфер с usage `STORAGE_BUFFER`.
        buffer: vk::Buffer,
        /// Размер буфера, байт (`range` дескриптора).
        size_bytes: u64,
        /// Шаг строки в БАЙТАХ (у AHB для RAW10 `stride` — байты строки).
        row_stride_bytes: u32,
        /// Ширина кадра, пиксели.
        width: u32,
        /// Высота кадра (число строк).
        height: u32,
        /// Как забрать буфер перед чтением.
        acquire: BufferAcquire,
        /// Куда отдать после чтения.
        release: ReleaseTarget,
    },
}

impl MosaicSource {
    /// Размер кадра в пикселях.
    fn size_px(&self) -> (u32, u32) {
        match *self {
            Self::SampledR16 { width, height, .. } | Self::PackedRaw10 { width, height, .. } => (width, height),
        }
    }
}

/// Проверяет параметры против источника. Все ошибки — `InvalidArgument`/`Unsupported` до записи команд.
pub(crate) fn validate(source: &MosaicSource, params: &MosaicPreviewParams) -> Result<(), GpuError> {
    let invalid = |what: String| GpuError::InvalidArgument { what };
    let (source_width, source_height) = source.size_px();
    let area = params.valid_area;
    if area.width < 2
        || area.height < 2
        || !area.left.is_multiple_of(2)
        || !area.top.is_multiple_of(2)
        || !area.width.is_multiple_of(2)
        || !area.height.is_multiple_of(2)
    {
        return Err(invalid(format!(
            "полезная область {area:?}: границы и размеры должны быть чётными (ячейка CFA 2x2), размер ≥ 2"
        )));
    }
    let right = u64::from(area.left) + u64::from(area.width);
    let bottom = u64::from(area.top) + u64::from(area.height);
    if right > u64::from(source_width) || bottom > u64::from(source_height) {
        return Err(invalid(format!(
            "полезная область {area:?} выходит за кадр {source_width}x{source_height}"
        )));
    }
    let highest_black = params.black_levels.iter().copied().max().unwrap_or(0);
    if params.white_level <= highest_black {
        return Err(invalid(format!(
            "белый уровень {} не больше чёрного {highest_black}",
            params.white_level
        )));
    }
    if let MosaicSource::PackedRaw10 {
        size_bytes,
        row_stride_bytes,
        ..
    } = *source
    {
        // Все прочитанные байты обязаны лежать внутри буфера: шейдер за границами не проверяет.
        let row_bytes = right.div_ceil(4) * 5;
        let required = (bottom - 1) * u64::from(row_stride_bytes) + row_bytes;
        if u64::from(row_stride_bytes) < row_bytes || required.next_multiple_of(4) > size_bytes {
            return Err(invalid(format!(
                "RAW10: шаг строки {row_stride_bytes} или размер буфера {size_bytes} не вмещают область {area:?} (нужно {required})"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw16(width: u32, height: u32) -> MosaicSource {
        MosaicSource::SampledR16 {
            view: vk::ImageView::null(),
            image: vk::Image::null(),
            width,
            height,
            acquire: ImageAcquire::FromForeignQueue,
            release: ReleaseTarget::ForeignQueue,
        }
    }

    fn params(area: ValidArea) -> MosaicPreviewParams {
        MosaicPreviewParams {
            valid_area: area,
            cfa: CfaOrder::Bggr,
            black_levels: [64; 4],
            white_level: 1023,
            sensor_rotation: QuarterTurns::None,
        }
    }

    const FULL: ValidArea = ValidArea {
        left: 0,
        top: 0,
        width: 8,
        height: 6,
    };

    #[test]
    fn valid_parameters_pass_and_each_violation_is_rejected() {
        assert!(validate(&raw16(8, 6), &params(FULL)).is_ok());
        for broken in [
            ValidArea { left: 1, ..FULL },
            ValidArea { width: 7, ..FULL },
            ValidArea { height: 0, ..FULL },
            ValidArea { top: 2, ..FULL },
        ] {
            let error = validate(&raw16(8, 6), &params(broken)).unwrap_err();
            assert!(
                matches!(error, GpuError::InvalidArgument { .. }),
                "{broken:?}: {error:?}"
            );
        }
        let mut no_range = params(FULL);
        no_range.white_level = 64;
        assert!(validate(&raw16(8, 6), &no_range).is_err(), "белый не выше чёрного");
    }

    #[test]
    fn raw10_buffer_must_hold_every_byte_the_shader_reads() {
        let packed = |size_bytes: u64, row_stride_bytes: u32| MosaicSource::PackedRaw10 {
            buffer: vk::Buffer::null(),
            size_bytes,
            row_stride_bytes,
            width: 8,
            height: 6,
            acquire: BufferAcquire::AlreadyOwned,
            release: ReleaseTarget::KeepOwned,
        };
        // 8 пикселей = 2 группы = 10 байт на строку; 6 строк со шагом 12.
        assert!(validate(&packed(12 * 6, 12), &params(FULL)).is_ok());
        assert!(
            validate(&packed(12 * 6 - 8, 12), &params(FULL)).is_err(),
            "буфер короче последней строки"
        );
        assert!(
            validate(&packed(1000, 8), &params(FULL)).is_err(),
            "шаг меньше байт строки"
        );
    }

    #[test]
    fn quarter_turns_round_trip_and_reject_out_of_range() {
        for count in 0..4 {
            assert_eq!(QuarterTurns::from_count(count).map(QuarterTurns::count), Some(count));
        }
        assert_eq!(QuarterTurns::from_count(4), None);
        assert!(QuarterTurns::Clockwise90.swaps_axes() && !QuarterTurns::Clockwise180.swaps_axes());
    }

    #[test]
    fn cfa_channels_have_one_red_one_blue_and_two_greens() {
        for order in [CfaOrder::Rggb, CfaOrder::Grbg, CfaOrder::Gbrg, CfaOrder::Bggr] {
            let channels = order.channels();
            let count = |wanted| channels.iter().filter(|channel| **channel == wanted).count();
            assert_eq!(
                (
                    count(CfaChannel::Red),
                    count(CfaChannel::Green),
                    count(CfaChannel::Blue)
                ),
                (1, 2, 1)
            );
        }
        assert_eq!(CfaOrder::Bggr.channels()[0], CfaChannel::Blue);
    }
}
