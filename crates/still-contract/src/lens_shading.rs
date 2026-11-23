//! Какая коррекция виньетирования (затенения объектива) реально применена к проявленному кадру снимка.
//!
//! # Зачем в контракте снимка
//! Решение 8 («честный RAW»): отсчёты в DNG остаются нетронутыми, а поправка, которую наш ISP применил к ЭТОМУ
//! кадру при проявлении JPEG/HEIF, должна поехать в DNG описанием (`OpcodeList2`, GainMap), чтобы конвертеры
//! проявили DNG так же ровно. Писатель файлов (`photo-output`) не знает ни про ISP, ни про его параметры, поэтому
//! рендер кладёт в факты кадра готовый ответ на вопрос «что применено», в словаре `camera-contract`.
//!
//! Тип нарочно НЕ повторяет типы `isp-model` (этот crate не видит `isp-model`): карта здесь — та же
//! [`LensShadingGainGrid`] (точки над полезной областью буфера, `[R, G_even, G_odd, B]`, усиления ≥ 1).

use std::sync::Arc;

use camera_contract::lens_shading::LensShadingGainGrid;

/// Почему к кадру коррекция не применялась (отражает причины пропуска стадии ISP).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LensShadingNotAppliedReason {
    /// У потока нет нашей калибровки виньетирования: углы остались как есть.
    NotCalibrated,
    /// Коррекцию выключило управление (конфиг или пользователь).
    DisabledByControl,
}

/// Коррекция виньетирования, применённая к проявленному кадру снимка.
#[derive(Debug, Clone, PartialEq)]
pub enum AppliedLensShading {
    /// Применена карта усилений ровно такой силы, с какой проявлялся кадр (если сила неполная — уже
    /// смешанная с единицей). Карта лежит над ПОЛЕЗНОЙ областью буфера, которую DNG хранит целиком.
    Applied(Arc<LensShadingGainGrid>),
    /// Не применялась: DNG описания коррекции не получает.
    NotApplied(LensShadingNotAppliedReason),
}

impl AppliedLensShading {
    /// Карта усилений, если коррекция применялась.
    pub fn grid(&self) -> Option<&Arc<LensShadingGainGrid>> {
        match self {
            Self::Applied(grid) => Some(grid),
            Self::NotApplied(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_is_present_only_when_applied() {
        let grid = Arc::new(LensShadingGainGrid::new(2, 2, vec![[1.0, 2.0, 3.0, 4.0]; 4]).unwrap());
        let applied = AppliedLensShading::Applied(Arc::clone(&grid));
        assert_eq!(applied.grid().map(|found| found.columns()), Some(2));
        for reason in [
            LensShadingNotAppliedReason::NotCalibrated,
            LensShadingNotAppliedReason::DisabledByControl,
        ] {
            assert!(AppliedLensShading::NotApplied(reason).grid().is_none());
        }
    }
}
