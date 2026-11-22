//! Параметры обработки кадра снимка из записей видоискателя (вынесено из `engine_steps` ради размера).
//!
//! Снимок «что было на экране» обязан повторить параметры СВОЕГО кадра (решение 72): белую и подтяжку `k`,
//! с которыми видоискатель его показал. Кадр со вспышкой берёт параметры иначе — `flash_frame`.

use camera_contract::frame_passport::RawFramePassport;
use still_contract::frame_choice::NoFrameReason;

use super::engine::StillEngine;
use super::feed::FrameStillParams;
use super::gpu::StillGpu;
use super::params_history::ParamsLookup;

impl<G: StillGpu> StillEngine<G> {
    /// Параметры кадра по истории видоискателя. Записи нет вовсе — «кадра нет» (затвор не освобождался).
    pub(super) fn viewfinder_params_for(
        &mut self,
        passport: &RawFramePassport,
    ) -> Result<FrameStillParams, NoFrameReason> {
        match self.params_history.lookup(passport.sensor_timestamp, &passport.stream) {
            ParamsLookup::Own(params) => {
                self.counters.params.own += 1;
                Ok(params)
            }
            ParamsLookup::NearestOfSameStream { params, distance_nanos } => {
                tracing::debug!(target: super::LOG_TARGET, distance_nanos, "у кадра снимка нет своей записи параметров");
                self.counters.params.nearest_of_same_stream += 1;
                Ok(params)
            }
            ParamsLookup::Missing => {
                self.counters.params.missing += 1;
                Err(NoFrameReason::FrameUnusable)
            }
        }
    }
}
