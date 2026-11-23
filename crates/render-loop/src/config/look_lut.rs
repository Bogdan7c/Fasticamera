//! Один источник лимита таблицы вида (LUT): `viewfinder.look_model.max_lut_size` (сессия 14a, решения 98, 178).
//!
//! Буферы таблицы выделяются в трёх местах `isp-vulkan` — слоты ISP видоискателя, слоты стенда V4 и проявитель
//! снимка, — и каждое имеет свой `look_lut` ([`LookLutLimits`]). Если бы они считались независимо, владелец мог бы
//! поднять предел модели (`max_lut_size`), а таблицу такого размера кадр видоискателя или снимок отвергал бы
//! «слишком большая». Поэтому:
//! * [`look_lut_limits`] — единственное место, где число превращается в [`LookLutLimits`];
//! * [`validate_single_source`] отвергает конфиг, где одно из трёх полей задано СВОИМ числом, не равным ни модели,
//!   ни умолчанию (иначе явное значение молча игнорировалось бы). Поле в умолчании — «не задано»: владелец меняет
//!   только `look_model.max_lut_size`, остальные три подхватывают его при сборке бэкенда;
//! * [`apply_single_source`] при сборке настроек бэкенда ставит лимит модели во все три места.

use isp_model::look::model::LookModelSettings;
use isp_vulkan::look::LookLutLimits;

use super::{StillDevelopConfig, ViewfinderConfig};

/// Лимит таблицы вида, который задаёт модель вида (`look_model.max_lut_size`).
pub fn look_lut_limits(model: &LookModelSettings) -> LookLutLimits {
    LookLutLimits {
        max_size: model.max_lut_size,
    }
}

/// Лимит таблицы задан не в модели вида, а в другом месте, и расходится с ней.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "{field}.max_size = {actual}, а viewfinder.look_model.max_lut_size = {from_model}: предел таблицы вида один, \
     задавайте его только в viewfinder.look_model.max_lut_size"
)]
pub struct LookLutLimitConflict {
    /// Поле конфига, которое расходится с моделью.
    pub field: &'static str,
    /// Что задано в этом поле.
    pub actual: u32,
    /// Что задано в модели вида.
    pub from_model: u32,
}

/// Три поля лимита (`viewfinder.isp.look_lut`, `viewfinder.headroom_bench.look_lut`, `still.photo.look_lut`) либо
/// равны пределу модели вида, либо не тронуты (умолчание — их подменит [`apply_single_source`]); своё число, не
/// равное ни тому ни другому, — конфликт.
pub(super) fn validate_single_source(
    viewfinder: &ViewfinderConfig,
    still: &StillDevelopConfig,
) -> Result<(), LookLutLimitConflict> {
    let from_model = look_lut_limits(&viewfinder.look_model);
    for (field, limits) in [
        ("viewfinder.isp.look_lut", viewfinder.isp.look_lut),
        ("viewfinder.headroom_bench.look_lut", viewfinder.headroom_bench.look_lut),
        ("still.photo.look_lut", still.photo.look_lut),
    ] {
        if limits != from_model && limits != LookLutLimits::default() {
            return Err(LookLutLimitConflict {
                field,
                actual: limits.max_size,
                from_model: from_model.max_size,
            });
        }
    }
    Ok(())
}

/// Ставит предел модели вида во все три места (видоискатель, стенд V4, снимок): при сборке настроек бэкенда.
pub(crate) fn apply_single_source(
    viewfinder: &ViewfinderConfig,
    still: &StillDevelopConfig,
) -> (ViewfinderConfig, StillDevelopConfig) {
    let limits = look_lut_limits(&viewfinder.look_model);
    let mut viewfinder = viewfinder.clone();
    viewfinder.isp.look_lut = limits;
    viewfinder.headroom_bench.look_lut = limits;
    let mut still = *still;
    still.photo.look_lut = limits;
    (viewfinder, still)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model_with_limit(max_lut_size: u32) -> LookModelSettings {
        LookModelSettings {
            max_lut_size,
            ..LookModelSettings::default()
        }
    }

    #[test]
    fn default_config_has_one_limit_everywhere() {
        let viewfinder = ViewfinderConfig::default();
        let still = StillDevelopConfig::default();
        assert_eq!(validate_single_source(&viewfinder, &still), Ok(()));
        assert_eq!(viewfinder.isp.look_lut, look_lut_limits(&viewfinder.look_model));
    }

    #[test]
    fn model_limit_reaches_viewfinder_bench_and_still() {
        let viewfinder = ViewfinderConfig {
            look_model: model_with_limit(17),
            ..ViewfinderConfig::default()
        };
        let (viewfinder, still) = apply_single_source(&viewfinder, &StillDevelopConfig::default());
        assert_eq!(viewfinder.isp.look_lut.max_size, 17);
        assert_eq!(viewfinder.headroom_bench.look_lut.max_size, 17);
        assert_eq!(still.photo.look_lut.max_size, 17);
        // Остальное не тронуто.
        assert_eq!(viewfinder.isp.frame_slots, ViewfinderConfig::default().isp.frame_slots);
        assert_eq!(still.photo.slice_rows, StillDevelopConfig::default().photo.slice_rows);
    }

    #[test]
    fn a_limit_set_outside_the_model_is_a_named_conflict_not_a_silent_override() {
        let still = StillDevelopConfig::default();
        let mut viewfinder = ViewfinderConfig::default();
        viewfinder.isp.look_lut.max_size = 9;
        assert_eq!(
            validate_single_source(&viewfinder, &still),
            Err(LookLutLimitConflict {
                field: "viewfinder.isp.look_lut",
                actual: 9,
                from_model: 65,
            })
        );
        let mut bench = ViewfinderConfig::default();
        bench.headroom_bench.look_lut.max_size = 9;
        assert_eq!(
            validate_single_source(&bench, &still).unwrap_err().field,
            "viewfinder.headroom_bench.look_lut"
        );
        let mut photo = StillDevelopConfig::default();
        photo.photo.look_lut.max_size = 9;
        let error = validate_single_source(&ViewfinderConfig::default(), &photo).unwrap_err();
        assert_eq!(error.field, "still.photo.look_lut");
        assert!(error.to_string().contains("look_model.max_lut_size"), "{error}");
        // Модель с другим пределом и нетронутыми (умолчание) полями — не конфликт: поля подхватят модель.
        let viewfinder = ViewfinderConfig {
            look_model: model_with_limit(17),
            ..ViewfinderConfig::default()
        };
        assert_eq!(
            validate_single_source(&viewfinder, &StillDevelopConfig::default()),
            Ok(())
        );
        // А своё число, не равное модели, — конфликт даже когда модель не в умолчании.
        let mut custom = viewfinder;
        custom.isp.look_lut.max_size = 9;
        assert_eq!(
            validate_single_source(&custom, &StillDevelopConfig::default())
                .unwrap_err()
                .from_model,
            17
        );
    }
}
