//! `HostRawFrame` на GPU: создание, источник нужной формы, возврат объектов после drop, отказ до GPU.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod common;

use common::{assert_no_validation_errors, gpu_context_or_skip};
use gpu_vulkan::raw_mosaic_preview::MosaicSource;
use gpu_vulkan::saturation::{HostRawFrame, RawFrameError};

#[test]
fn source_describes_the_uploaded_frame_and_objects_are_released() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    {
        let bytes = vec![7_u8; 12 * 4];
        let frame = HostRawFrame::new(&context, 8, 4, 12, &bytes).unwrap();
        let MosaicSource::PackedRaw10 {
            size_bytes,
            row_stride_bytes,
            width,
            height,
            ..
        } = frame.source()
        else {
            panic!("ожидался упакованный RAW10");
        };
        assert_eq!((size_bytes, row_stride_bytes, width, height), (48, 12, 8, 4));
        assert_ne!(context.live_objects(), baseline, "буфер должен существовать");
    }
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}

#[test]
fn wrong_sizes_fail_before_any_gpu_object_is_created() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    assert!(matches!(
        HostRawFrame::new(&context, 0, 4, 12, &[0; 48]),
        Err(RawFrameError::ZeroSize { .. })
    ));
    assert!(matches!(
        HostRawFrame::new(&context, 8, 4, 12, &[0; 47]),
        Err(RawFrameError::ByteCountMismatch { .. })
    ));
    assert_eq!(context.live_objects(), baseline);
}
