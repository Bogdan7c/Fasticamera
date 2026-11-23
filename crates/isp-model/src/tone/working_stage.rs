//! Стадии основы тона над изображениями в рабочем пространстве `W` — ОДНИ для всех моделей (AgX, ACES, filmic).
//!
//! Модель основы даёт попиксельные формулы через контракт [`WorkingToneBase`]; стадии ниже не знают, какая именно
//! модель за ним стоит, поэтому эталонная цепочка ([`crate::pipeline`]) не копируется на модель.

use super::aces::AcesBase;
use super::agx::AgxBase;
use super::filmic::FilmicBase;
use crate::image::{
    DisplayGamut, DisplayLinearImage, RgbImage, WorkingDisplayImage, WorkingSceneImage, XyzD50Image, space,
};
use crate::white_level::WhiteLevelPlane;

/// Контракт модели основы тона, работающей в `W` (все модели, кроме прежней кривой).
///
/// Реализуют [`AgxBase`], [`super::aces::AcesBase`], [`super::filmic::FilmicBase`]; выбирает реализацию
/// [`super::base::ResolvedToneBase::working_stage`].
pub trait WorkingToneBase {
    /// Гамма выхода, под которую посчитана матрица `W → выход`.
    fn output_gamut(&self) -> DisplayGamut;

    /// `XYZ D50 → W` одного пикселя.
    fn xyz_d50_to_working(&self, xyz_d50: [f32; 3]) -> [f32; 3];

    /// Основа для пикселя сцены в `W` со своим белым `white` → линейный свет экрана в `W`.
    fn map_scene_pixel(&self, scene: [f32; 3], white: f32) -> [f32; 3];

    /// Последний шаг: `W → цвета выхода`, мягкое сжатие охвата выхода, обрезка 0…1.
    fn to_output(&self, display_working: [f32; 3]) -> [f32; 3];
}

impl WorkingToneBase for AgxBase {
    fn output_gamut(&self) -> DisplayGamut {
        AgxBase::output_gamut(self)
    }

    fn xyz_d50_to_working(&self, xyz_d50: [f32; 3]) -> [f32; 3] {
        AgxBase::xyz_d50_to_working(self, xyz_d50)
    }

    fn map_scene_pixel(&self, scene: [f32; 3], white: f32) -> [f32; 3] {
        AgxBase::map_scene_pixel(self, scene, white)
    }

    fn to_output(&self, display_working: [f32; 3]) -> [f32; 3] {
        AgxBase::to_output(self, display_working)
    }
}

impl WorkingToneBase for AcesBase {
    fn output_gamut(&self) -> DisplayGamut {
        AcesBase::output_gamut(self)
    }

    fn xyz_d50_to_working(&self, xyz_d50: [f32; 3]) -> [f32; 3] {
        AcesBase::xyz_d50_to_working(self, xyz_d50)
    }

    fn map_scene_pixel(&self, scene: [f32; 3], white: f32) -> [f32; 3] {
        AcesBase::map_scene_pixel(self, scene, white)
    }

    fn to_output(&self, display_working: [f32; 3]) -> [f32; 3] {
        AcesBase::to_output(self, display_working)
    }
}

impl WorkingToneBase for FilmicBase {
    fn output_gamut(&self) -> DisplayGamut {
        FilmicBase::output_gamut(self)
    }

    fn xyz_d50_to_working(&self, xyz_d50: [f32; 3]) -> [f32; 3] {
        FilmicBase::xyz_d50_to_working(self, xyz_d50)
    }

    fn map_scene_pixel(&self, scene: [f32; 3], white: f32) -> [f32; 3] {
        FilmicBase::map_scene_pixel(self, scene, white)
    }

    fn to_output(&self, display_working: [f32; 3]) -> [f32; 3] {
        FilmicBase::to_output(self, display_working)
    }
}

/// XYZ D50 → линейный Rec. 2020 (Брэдфорд D50 → D65 внутри матрицы).
pub fn xyz_d50_to_working_scene<B: WorkingToneBase + ?Sized>(image: &XyzD50Image, base: &B) -> WorkingSceneImage {
    image.map_into(space::WorkingScene, |pixel| base.xyz_d50_to_working(pixel))
}

/// Основа тона по всему изображению: белый каждого пикселя берётся из плоскости `white_levels`.
///
/// Размеры обязаны совпадать (плоскость строится по тому же демозаику); при расхождении — `None`.
pub fn apply_working_base<B: WorkingToneBase + ?Sized>(
    image: &WorkingSceneImage,
    white_levels: &WhiteLevelPlane,
    base: &B,
) -> Option<WorkingDisplayImage> {
    if image.size() != white_levels.size() {
        return None;
    }
    let pixels = image
        .pixels()
        .iter()
        .zip(white_levels.levels())
        .map(|(&pixel, &white)| base.map_scene_pixel(pixel, white))
        .collect();
    Some(RgbImage::from_stage_output(image.size(), pixels, space::WorkingDisplay))
}

/// Последний шаг цепочки: рабочее пространство → цвета выхода, мягкое сжатие охвата, обрезка 0…1.
pub fn working_display_to_output<B: WorkingToneBase + ?Sized>(
    image: &WorkingDisplayImage,
    base: &B,
) -> DisplayLinearImage {
    let gamut = base.output_gamut();
    image.map_into(space::DisplayLinear { gamut }, |pixel| base.to_output(pixel))
}
