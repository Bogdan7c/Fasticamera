//! Цветовая наука: камера → CIE XYZ D50 по матрицам DNG → основные цвета экрана.
//!
//! - [`matrix`] — матрицы 3×3 (подготовка в f64, применение в f32).
//! - [`chromaticity`] — координаты цветности, основные цвета sRGB/Display P3, адаптация Брэдфорда.
//! - [`temperature`] — коррелированная цветовая температура (метод Робертсона).
//! - [`white_point`] — белая точка как «температура + Duv», привязка к кривой Планка.
//! - [`dng`] — спецификация DNG, глава «Mapping Camera Color Space to CIE XYZ Space»:
//!   интерполяция матриц по температуре и поиск белой точки по нейтрали камеры.
//! - [`stage`] — попиксельные стадии цвета над изображениями.

pub mod chromaticity;
pub mod dng;
pub mod matrix;
pub mod stage;
pub mod temperature;
pub mod white_point;
