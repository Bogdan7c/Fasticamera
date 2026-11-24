//! Push-константы проходов одной полосы проявления.

use ash::vk;

use super::slicing::{DevelopmentPlan, SlicePlan};
use crate::quality::frame_params::CfaCode;
use crate::quality::pass::PassPushConstants;

/// Push-константы трёх видов проходов одной полосы (раскладка `PlaneGeometry`; индексы < 2^24 точны в f32).
pub(super) struct SlicePushes {
    pub(super) levels: PassPushConstants,
    pub(super) geometry: PassPushConstants,
    pub(super) output: PassPushConstants,
}

impl SlicePushes {
    pub(super) fn new(plan: &DevelopmentPlan, slice: &SlicePlan, cfa: CfaCode, source_width: u32) -> Self {
        let extent = vk::Extent2D {
            width: plan.region_width,
            height: slice.region_rows,
        };
        let geometry = PassPushConstants::geometry(extent, cfa);
        Self {
            levels: PassPushConstants {
                pass_parameters: [
                    plan.region_left as f32,
                    slice.region_top as f32,
                    source_width as f32,
                    0.0,
                ],
                ..geometry
            },
            output: PassPushConstants {
                pass_parameters: [
                    plan.window_left as f32,
                    slice.first_output_row as f32,
                    slice.output_rows as f32,
                    slice.destination_row as f32,
                ],
                ..geometry
            },
            geometry,
        }
    }
}
