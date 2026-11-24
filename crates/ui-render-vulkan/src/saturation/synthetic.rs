//! Подложка из синтетического фона тестов и стенда (фича `test-support`).

use gpu_vulkan::backdrop::{BackdropFrameNumber, BackdropSource};

use super::scene::BackdropFeed;
use crate::testing::SyntheticBackdrop;

impl BackdropFeed for SyntheticBackdrop {
    fn backdrop_for_frame(&self, frame: BackdropFrameNumber) -> BackdropSource {
        self.source(frame.0)
    }
}
