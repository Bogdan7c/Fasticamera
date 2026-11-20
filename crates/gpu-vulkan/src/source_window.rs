//! [`SourceWindow`] — какую часть картинки-источника показать и как: содержимое в левом верхнем
//! прямоугольнике картинки, зеркало по горизонтали и увеличение вокруг центра содержимого.
//!
//! # Зачем (сессия 13)
//! * **Зум без пересоздания картинок.** Выход ISP живёт в картинке постоянной ёмкости (кадр при
//!   зуме 1×), а реальное содержимое кадра при большем зуме меньше и лежит в левом верхнем
//!   прямоугольнике. Компоновщик и подложка стекла обязаны читать ТОЛЬКО этот прямоугольник — иначе на
//!   экран попали бы остатки прежнего (более широкого) кадра.
//! * **Зеркало превью фронтальной камеры.** Зеркало делается здесь, а не через `SurfaceTransform`:
//!   тот описывает поворот дисплея, и компоновщик отвергает его зеркальные варианты.
//! * **Увеличение удержанного кадра** (решение 112): пока палец крутит колесо во время смены объектива,
//!   удержанная картинка масштабируется вокруг центра. Не меньше 1 — чёрных полей не бывает.
//!
//! Окно — чистые данные: одна и та же [`TexelMapping`] кладётся в push-константы компоновщика, первой
//! ступени пирамиды размытия и шейдера стекла, а [`TexelMapping::texel_at`] — её CPU-эталон для тестов.
//!
//! # Формула
//! Для точки назначения `relative ∈ [0, 1)²` (доля прямоугольника, уже без зеркала):
//! `texel = origin + relative · step`, затем зажим в `[0.5, content − 0.5]` по каждой оси. Зажим на
//! полтекселя внутрь не даёт билинейной выборке на краю содержимого подмешать соседние (чужие) текселя
//! картинки ёмкости. Картинка без зума, зеркала и увеличения даёт `origin = 0`, `step = content`.

use ash::vk;

use crate::error::GpuError;

/// Зеркалить ли картинку по горизонтали (на экране, после всех поворотов).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HorizontalMirror {
    /// Как есть.
    #[default]
    Off,
    /// Слева направо наоборот.
    On,
}

/// Окно чтения картинки-источника (см. модуль).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceWindow {
    /// Размер содержимого (левый верхний прямоугольник картинки), текселей.
    content: vk::Extent2D,
    mirror: HorizontalMirror,
    /// Увеличение вокруг центра содержимого, ≥ 1.
    magnification: f32,
}

/// Окно, переведённое в числа для шейдера: `texel = origin + relative · step`, затем зажим по `content`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TexelMapping {
    /// Текселя источника в точке `relative = (0, 0)`.
    pub origin: [f32; 2],
    /// Приращение текселей на единицу `relative` (отрицательное по x при зеркале).
    pub step: [f32; 2],
    /// Размер содержимого, текселей: зажим выборки в `[0.5, content − 0.5]`.
    pub content: [f32; 2],
}

impl SourceWindow {
    /// Вся картинка `extent` целиком, без зеркала и увеличения.
    pub fn full(extent: vk::Extent2D) -> Self {
        Self::of_content(extent)
    }

    /// Содержимое размера `content` в левом верхнем углу картинки (ёмкость может быть больше).
    pub fn of_content(content: vk::Extent2D) -> Self {
        Self {
            content,
            mirror: HorizontalMirror::Off,
            magnification: 1.0,
        }
    }

    /// То же окно с зеркалом.
    pub fn with_mirror(mut self, mirror: HorizontalMirror) -> Self {
        self.mirror = mirror;
        self
    }

    /// То же окно с увеличением вокруг центра. Меньше 1 или не число — отказ (чёрные поля недопустимы).
    pub fn with_magnification(mut self, magnification: f32) -> Result<Self, GpuError> {
        if !(magnification.is_finite() && magnification >= 1.0) {
            return Err(GpuError::InvalidArgument {
                what: format!("увеличение окна источника {magnification}: нужно конечное число не меньше 1"),
            });
        }
        self.magnification = magnification;
        Ok(self)
    }

    /// Размер содержимого.
    pub fn content(&self) -> vk::Extent2D {
        self.content
    }

    /// Зеркало.
    pub fn mirror(&self) -> HorizontalMirror {
        self.mirror
    }

    /// Увеличение вокруг центра (≥ 1).
    pub fn magnification(&self) -> f32 {
        self.magnification
    }

    /// Окно годится для картинки размера `image`: содержимое непусто и не больше картинки.
    pub fn validate_within(&self, image: vk::Extent2D) -> Result<(), GpuError> {
        if self.content.width == 0 || self.content.height == 0 {
            return Err(GpuError::InvalidArgument {
                what: format!("пустое содержимое окна источника {:?}", self.content),
            });
        }
        if self.content.width > image.width || self.content.height > image.height {
            return Err(GpuError::InvalidArgument {
                what: format!("содержимое {:?} больше картинки {image:?}", self.content),
            });
        }
        Ok(())
    }

    /// Окно в числах для шейдера.
    pub fn mapping(&self) -> TexelMapping {
        let content = [self.content.width as f32, self.content.height as f32];
        // Видимая часть содержимого при увеличении `m` — центральная 1/m его размера.
        let visible = content.map(|side| side / self.magnification);
        let first = [(content[0] - visible[0]) / 2.0, (content[1] - visible[1]) / 2.0];
        let (origin_x, step_x) = match self.mirror {
            HorizontalMirror::Off => (first[0], visible[0]),
            // Зеркало: правый край видимой части — в левом краю назначения, шаг отрицательный.
            HorizontalMirror::On => (first[0] + visible[0], -visible[0]),
        };
        TexelMapping {
            origin: [origin_x, first[1]],
            step: [step_x, visible[1]],
            content,
        }
    }
}

impl TexelMapping {
    /// CPU-эталон шейдера: текселя источника (непрерывные координаты, центр первого текселя — 0.5)
    /// для точки назначения `relative`.
    pub fn texel_at(&self, relative: [f32; 2]) -> [f32; 2] {
        let raw = [
            self.origin[0] + relative[0] * self.step[0],
            self.origin[1] + relative[1] * self.step[1],
        ];
        [0, 1].map(|axis| raw[axis].clamp(0.5, (self.content[axis] - 0.5).max(0.5)))
    }

    /// Шесть чисел для push-констант: `origin.xy, step.xy`, затем `content.xy`.
    pub fn to_floats(&self) -> [f32; 6] {
        [
            self.origin[0],
            self.origin[1],
            self.step[0],
            self.step[1],
            self.content[0],
            self.content[1],
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extent(width: u32, height: u32) -> vk::Extent2D {
        vk::Extent2D { width, height }
    }

    #[test]
    fn plain_window_maps_the_whole_content_one_to_one() {
        let mapping = SourceWindow::of_content(extent(8, 4)).mapping();
        assert_eq!(mapping.origin, [0.0, 0.0]);
        assert_eq!(mapping.step, [8.0, 4.0]);
        assert_eq!(mapping.texel_at([0.5, 0.5]), [4.0, 2.0]);
    }

    #[test]
    fn mirror_reverses_only_the_horizontal_axis() {
        let mapping = SourceWindow::of_content(extent(8, 4))
            .with_mirror(HorizontalMirror::On)
            .mapping();
        // Левый край назначения читает правый край содержимого (с зажимом на полтекселя внутрь).
        assert_eq!(mapping.texel_at([0.0, 0.0]), [7.5, 0.5]);
        assert_eq!(mapping.texel_at([1.0, 1.0]), [0.5, 3.5]);
        assert_eq!(mapping.texel_at([0.25, 0.5]), [6.0, 2.0]);
    }

    #[test]
    fn magnification_keeps_the_centre_and_never_leaves_the_content() {
        let window = SourceWindow::of_content(extent(8, 4)).with_magnification(2.0).unwrap();
        let mapping = window.mapping();
        assert_eq!(mapping.origin, [2.0, 1.0], "видна центральная половина");
        assert_eq!(mapping.step, [4.0, 2.0]);
        assert_eq!(mapping.texel_at([0.5, 0.5]), [4.0, 2.0], "центр остался центром");
        for relative in [[0.0, 0.0], [1.0, 1.0], [1.0, 0.0], [0.0, 1.0]] {
            let texel = mapping.texel_at(relative);
            assert!(
                (0.5..=7.5).contains(&texel[0]) && (0.5..=3.5).contains(&texel[1]),
                "{texel:?}"
            );
        }
    }

    #[test]
    fn edge_texels_are_clamped_half_a_texel_inside_the_content() {
        // Ёмкость картинки больше содержимого: за краем содержимого лежат чужие текселя.
        let mapping = SourceWindow::of_content(extent(4, 4)).mapping();
        assert_eq!(mapping.texel_at([1.0, 1.0]), [3.5, 3.5]);
        assert_eq!(mapping.texel_at([0.0, 0.0]), [0.5, 0.5]);
    }

    #[test]
    fn invalid_magnification_and_oversized_content_are_rejected() {
        let window = SourceWindow::of_content(extent(8, 4));
        for bad in [0.99, 0.0, f32::NAN, f32::INFINITY, -2.0] {
            assert!(window.with_magnification(bad).is_err(), "{bad}");
        }
        assert!(window.validate_within(extent(8, 4)).is_ok());
        assert!(window.validate_within(extent(16, 8)).is_ok());
        assert!(
            window.validate_within(extent(7, 4)).is_err(),
            "содержимое шире картинки"
        );
        assert!(
            SourceWindow::of_content(extent(0, 4))
                .validate_within(extent(8, 4))
                .is_err()
        );
    }
}
