//! Сборка работ «вид картинки» для стенда насыщения (сессия 14o): `look_cost` и `look_parity`.
//!
//! Живёт дочерним модулем исполнителя: ему нужны его приватные поля и `BenchWork`, а сам `gpu_executor.rs` уже
//! велик (правило размера модулей). Что за вид показать, решает каталог и `look_variant`; здесь — только «рецепт вида →
//! параметры → слитый проход видоискателя».

use look_contract::LookRecipe;

use super::*;
use crate::saturation_bench::look_variant::ParityLook;

impl GpuWorkloadExecutor {
    /// Слитый проход видоискателя продуктового размера с видом `look` вместо активного: остальные параметры — продукта
    /// (сенсор, карта теней, охват), вариант прохода — продуктовый.
    pub(super) fn build_viewfinder_look(&self, look: &LookRecipe) -> Result<Box<dyn BenchWork>, BuildError> {
        let params = self.inputs.params_with_look(PipelineKind::Viewfinder, look)?;
        let config = viewfinder_variant::configure(ViewfinderVariant::Product, self.viewfinder_config());
        let work = ViewfinderSaturation::new_synthetic(&self.context, config, params)?;
        Ok(Box::new(ViewfinderWork::new(work)))
    }

    /// Работа паритета: тот же проход с видом линейки (цена строки в выводы не идёт), числа паритета добавляются
    /// после прогона и освобождения работы.
    pub(super) fn build_look_parity(&self, look: ParityLook) -> Result<Box<dyn BenchWork>, BuildError> {
        let recipe = look
            .recipe(&self.inputs)
            .map_err(|reason| BuildError(WorkloadFailure::new(FailureKind::BuildFailed, reason)))?;
        self.build_viewfinder_look(&recipe)
    }
}
