//! Группа стекла: несколько форм, сливающихся в одно стекло.

use crate::color::Opacity;
use crate::draw_list::bounds::transformed_bounds;
use crate::geometry::{PxPoint, PxRect, PxVector};
use crate::glass::GlassParamsPx;
use crate::glass::reference::PressInput;
use crate::shape::{ShapeGeometry, corrected_distance, smooth_union_with_weights};
use crate::transform::Affine2;

/// Предел числа форм в одной группе — размер массива в шейдере.
///
/// Инженерная константа: массив членов лежит в uniform-буфере (около 100 байт на член: размер,
/// радиус, показатель, обратная матрица, нажатие), 16 членов = 1,6 КиБ, с большим запасом в пределах
/// гарантированного размера uniform-буфера Vulkan (16 КиБ). Хватает на самую большую связную группу
/// экрана камеры (нижний ряд кнопок плюс переключатель режимов). Больше — ошибка валидации,
/// а не молчаливая обрезка. Подгонка, не Apple.
pub const MAX_GLASS_GROUP_MEMBERS: usize = 16;

/// Нажатие одного члена группы.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PressState {
    /// 0 — не нажат, 1 — нажат полностью.
    pub amount: f32,
    /// Точка касания в локальных координатах члена (центр = 0).
    pub touch_point_local: PxPoint,
}

impl PressState {
    /// Не нажат.
    pub const RELEASED: PressState = PressState {
        amount: 0.0,
        touch_point_local: PxPoint::ORIGIN,
    };
}

/// Одна форма группы.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlassMember {
    pub geometry: ShapeGeometry,
    /// Локальные координаты (центр = 0) → экран. Уже включает масштаб нажатия
    /// ([`crate::glass::PressParamsPx::scale_for`]): рендер его не добавляет.
    pub transform: Affine2,
    pub press: PressState,
}

impl GlassMember {
    /// Точка касания члена в экранных пикселях.
    pub fn touch_point_px(&self) -> PxPoint {
        self.transform.apply_point(self.press.touch_point_local)
    }

    /// Границы формы на экране (без запасов).
    pub fn shape_bounds_px(&self) -> Option<PxRect> {
        let local = PxRect::from_center(PxPoint::ORIGIN, self.geometry.extent());
        transformed_bounds(&self.transform, &local)
    }
}

/// Группа стекла: одним проходом над одним фоном. Слияние членов — `smooth_union` радиусом
/// `merge_radius_px` (у Apple — `spacing` контейнера, `glass.md` раздел 2).
#[derive(Debug, Clone, PartialEq)]
pub struct GlassGroupItem {
    pub params: GlassParamsPx,
    pub members: Vec<GlassMember>,
    /// Радиус слияния, px; 0 — формы не сливаются.
    pub merge_radius_px: f32,
    /// Материализация 0..1 (Apple 219, 2:55): сила линзирования, НЕ прозрачность.
    pub effect: f32,
    pub opacity: Opacity,
}

/// Поле слитой формы в точке экрана.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroupField {
    /// Исправленное расстояние до слитой формы, px (внутри < 0).
    pub distance_px: f32,
    /// Единичная нормаль наружу (нулевая, если направления нет).
    pub normal_out: PxVector,
}

impl GlassGroupItem {
    /// Границы области, где стекло рисуется: формы всех членов, раздутые на запас под слияние
    /// (smooth-min выпячивает форму не более чем на `merge_radius/4`; берём `merge_radius`),
    /// тень, вынос линзы (смещение выборки наружу) и свечение нажатия. Рендер рисует стекло
    /// только внутри этих границ. `None` — группа пуста или преобразование не конечно.
    pub fn bounds_px(&self) -> Option<PxRect> {
        let mut bounds: Option<PxRect> = None;
        for member in &self.members {
            let member_bounds = member.shape_bounds_px()?;
            bounds = Some(match bounds {
                Some(acc) => acc.union(&member_bounds),
                None => member_bounds,
            });
        }
        let bounds = bounds?;
        let params = &self.params;
        let margin = self
            .merge_radius_px
            .max(params.shadow.radius_px + params.shadow.offset_px)
            .max(params.refraction_strength_px * (1.0 + params.press.refraction_boost))
            .max(params.press.glow_radius_px);
        Some(bounds.expanded(margin))
    }

    /// Нажатие группы для пикселя: у `shade_glass_pixel` одно нажатие на пиксель, а у членов оно
    /// своё. Берётся член с НАИБОЛЬШЕЙ силой нажатия (при равенстве — первый в списке): его сила и
    /// точка касания (экранные px) управляют усилением преломления и свечением всей группы.
    /// Свечение растекается на соседние формы, потому что точка касания и пиксель живут в одной
    /// экранной системе (Apple 219, 12:05). Нет членов — [`PressInput::NONE`]. Этим правилом
    /// пользуется и рендер при упаковке группы.
    pub fn dominant_press(&self) -> PressInput {
        let mut dominant: Option<&GlassMember> = None;
        for member in &self.members {
            // Строго «больше»: при равенстве остаётся первый.
            if dominant.is_none_or(|current| member.press.amount > current.press.amount) {
                dominant = Some(member);
            }
        }
        dominant.map_or(PressInput::NONE, |member| PressInput {
            amount: member.press.amount,
            touch_point_px: member.touch_point_px(),
        })
    }

    /// Эталонное поле слитой формы в экранной точке: расстояния членов (исправленные, в px)
    /// сливаются `smooth_union`, нормали — теми же весами. `Ok(None)` — членов нет;
    /// ошибка — вырожденное преобразование члена.
    pub fn field_at(&self, screen_point: PxPoint) -> Result<Option<GroupField>, crate::transform::AffineError> {
        let mut folded: Option<(f32, PxVector)> = None;
        for member in &self.members {
            let (distance, normal) = member_field(member, screen_point)?;
            folded = Some(match folded {
                None => (distance, normal),
                Some((acc_distance, acc_normal)) => {
                    let merged = smooth_union_with_weights(acc_distance, distance, self.merge_radius_px);
                    // Градиент слитого поля — взвешенная сумма градиентов (аналитическая производная smooth-min).
                    let blended = acc_normal * merged.weight_first + normal * merged.weight_second;
                    (merged.distance, blended)
                }
            });
        }
        Ok(folded.map(|(distance, blended)| GroupField {
            distance_px: distance,
            normal_out: blended.normalized().unwrap_or(PxVector::ZERO),
        }))
    }
}

/// Исправленное расстояние (px) и единичная нормаль члена в экранной точке.
fn member_field(member: &GlassMember, screen_point: PxPoint) -> Result<(f32, PxVector), crate::transform::AffineError> {
    let to_local = member.transform.inverse()?;
    let local_point = to_local.apply_point(screen_point);
    let sample = corrected_distance(&member.geometry, local_point);
    // Градиент в экранных координатах: (L⁻¹)ᵀ·∇_локальный; его длина — 1/масштаб. Деление на неё
    // переводит расстояние из локальных единиц в пиксели экрана (первый порядок).
    let local_gradient = sample.normal_out;
    let screen_gradient = to_local.apply_gradient(local_gradient);
    match screen_gradient.normalized() {
        Some(normal) => Ok((sample.distance / screen_gradient.length(), normal)),
        None => Ok((sample.distance, PxVector::ZERO)),
    }
}
