//! Подделки платформы: хранилище, кодеры, JPEG, форматы, часы, каталог камер.

use std::ffi::c_void;
use std::io::Write;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use camera_contract::ids::CameraId;
use photo_output::dng::{DngError, DngInputs};
use photo_output::exif::{CameraDescription, CaptureMoment, ExifError, ExifInputs, RotationCarrier};
use photo_output::jpeg_exif::JpegExifError;
use photo_output::service::{CameraCatalog, StillFileFormats, WallClock};
use photo_output::tiff::TiffError;
use shell_contract::still_media::{
    EncodeError, HeifEncodeSettings, HeifEncoderService, HeifSurfaceEncoder, JpegCompressor, MediaStoreError,
    MediaStoreService, PendingMedia, PendingMediaRequest, PublishedMedia,
};
use shell_contract::surface::{NativeSurface, PlatformWindow, SurfaceGeneration};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Что стало с черновиком.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DraftState {
    Pending,
    Published,
    Discarded,
    /// Черновик выброшен без явного итога (в платформе это тоже удаление, но служба так делать не должна).
    Abandoned,
}

#[derive(Debug, Clone)]
pub struct DraftRecord {
    pub request: PendingMediaRequest,
    pub bytes: Vec<u8>,
    pub state: DraftState,
}

/// Журнал хранилища.
#[derive(Default)]
pub struct MediaLog {
    drafts: Mutex<Vec<DraftRecord>>,
}

impl MediaLog {
    pub fn drafts(&self) -> Vec<DraftRecord> {
        lock(&self.drafts).clone()
    }

    /// Черновик по расширению имени (`HEIC`, `JPG`, `DNG`).
    pub fn by_extension(&self, extension: &str) -> Option<DraftRecord> {
        self.drafts()
            .into_iter()
            .find(|draft| draft.request.display_name.ends_with(extension))
    }

    pub fn published_count(&self) -> usize {
        self.count(&DraftState::Published)
    }

    pub fn discarded_count(&self) -> usize {
        self.count(&DraftState::Discarded)
    }

    pub fn pending_count(&self) -> usize {
        self.count(&DraftState::Pending)
    }

    fn count(&self, state: &DraftState) -> usize {
        self.drafts().iter().filter(|draft| &draft.state == state).count()
    }

    fn set_state(&self, index: usize, state: DraftState) {
        if let Some(draft) = lock(&self.drafts).get_mut(index) {
            draft.state = state;
        }
    }
}

/// Хранилище-подделка.
#[derive(Default)]
pub struct FakeStore {
    pub log: Arc<MediaLog>,
    fail_create_extension: Mutex<Option<String>>,
    fail_publish_extension: Mutex<Option<String>>,
    panic_on_create: AtomicBool,
}

impl FakeStore {
    /// Следующее создание черновика паникует (изображает баг в платформенном слое).
    pub fn panic_on_create(&self) {
        self.panic_on_create.store(true, Ordering::SeqCst);
    }

    /// Создание черновика с таким расширением будет отказывать.
    pub fn fail_create_for(&self, extension: &str) {
        *lock(&self.fail_create_extension) = Some(extension.to_owned());
    }

    /// Публикация черновика с таким расширением будет отказывать.
    pub fn fail_publish_for(&self, extension: &str) {
        *lock(&self.fail_publish_extension) = Some(extension.to_owned());
    }
}

impl MediaStoreService for FakeStore {
    fn create_pending(&self, request: &PendingMediaRequest) -> Result<Box<dyn PendingMedia>, MediaStoreError> {
        assert!(
            !self.panic_on_create.load(Ordering::SeqCst),
            "тест: паника платформенного слоя"
        );
        let extension = |slot: &Mutex<Option<String>>| {
            lock(slot)
                .as_deref()
                .is_some_and(|extension| request.display_name.ends_with(extension))
        };
        if extension(&self.fail_create_extension) {
            return Err(MediaStoreError::Platform("диск полон".to_owned()));
        }
        let mut drafts = lock(&self.log.drafts);
        let index = drafts.len();
        drafts.push(DraftRecord {
            request: request.clone(),
            bytes: Vec::new(),
            state: DraftState::Pending,
        });
        Ok(Box::new(FakeDraft {
            index,
            uri: format!("content://fake/{}", request.display_name),
            log: Arc::clone(&self.log),
            publish_fails: extension(&self.fail_publish_extension),
            resolved: false,
        }))
    }
}

struct FakeDraft {
    index: usize,
    uri: String,
    log: Arc<MediaLog>,
    publish_fails: bool,
    resolved: bool,
}

impl PendingMedia for FakeDraft {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<(), MediaStoreError> {
        if let Some(draft) = lock(&self.log.drafts).get_mut(self.index) {
            draft.bytes.extend_from_slice(bytes);
        }
        Ok(())
    }

    fn publish(mut self: Box<Self>) -> Result<PublishedMedia, MediaStoreError> {
        self.resolved = true;
        if self.publish_fails {
            // Платформа при отказе публикации убирает черновик сама.
            self.log.set_state(self.index, DraftState::Discarded);
            return Err(MediaStoreError::Platform("публикация отклонена".to_owned()));
        }
        self.log.set_state(self.index, DraftState::Published);
        Ok(PublishedMedia { uri: self.uri.clone() })
    }

    fn discard(mut self: Box<Self>) {
        self.resolved = true;
        self.log.set_state(self.index, DraftState::Discarded);
    }
}

impl Drop for FakeDraft {
    fn drop(&mut self) {
        if !self.resolved {
            self.log.set_state(self.index, DraftState::Abandoned);
        }
    }
}

/// Что случилось с кодером.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncoderEvent {
    Finished(Duration),
    Aborted,
}

/// Окно-пустышка для поверхности кодера.
struct FakeWindow;

impl PlatformWindow for FakeWindow {
    fn raw_window(&self) -> NonNull<c_void> {
        NonNull::dangling()
    }
}

/// Фабрика кодеров HEIF.
#[derive(Default)]
pub struct FakeHeif {
    opens: Mutex<Vec<(HeifEncodeSettings, String)>>,
    events: Arc<Mutex<Vec<EncoderEvent>>>,
    fail_open: AtomicBool,
    withhold_surface: AtomicBool,
    fail_finish: AtomicBool,
}

impl FakeHeif {
    pub fn fail_open(&self) {
        self.fail_open.store(true, Ordering::SeqCst);
    }

    pub fn withhold_surface(&self) {
        self.withhold_surface.store(true, Ordering::SeqCst);
    }

    pub fn fail_finish(&self) {
        self.fail_finish.store(true, Ordering::SeqCst);
    }

    /// Настройки и адрес черновика каждого открытия.
    pub fn opens(&self) -> Vec<(HeifEncodeSettings, String)> {
        lock(&self.opens).clone()
    }

    pub fn events(&self) -> Vec<EncoderEvent> {
        lock(&self.events).clone()
    }
}

impl HeifEncoderService for FakeHeif {
    fn open(
        &self,
        target: &dyn PendingMedia,
        settings: &HeifEncodeSettings,
    ) -> Result<Box<dyn HeifSurfaceEncoder>, EncodeError> {
        if self.fail_open.load(Ordering::SeqCst) {
            return Err(EncodeError::Platform("кодер занят".to_owned()));
        }
        lock(&self.opens).push((settings.clone(), target.uri().to_owned()));
        Ok(Box::new(FakeEncoder {
            surface: (!self.withhold_surface.load(Ordering::SeqCst))
                .then(|| NativeSurface::new(SurfaceGeneration::new(1), Box::new(FakeWindow))),
            events: Arc::clone(&self.events),
            finish_fails: self.fail_finish.load(Ordering::SeqCst),
        }))
    }
}

struct FakeEncoder {
    surface: Option<NativeSurface>,
    events: Arc<Mutex<Vec<EncoderEvent>>>,
    finish_fails: bool,
}

impl HeifSurfaceEncoder for FakeEncoder {
    fn take_input_surface(&mut self) -> Option<NativeSurface> {
        self.surface.take()
    }

    fn finish(self: Box<Self>, timeout: Duration) -> Result<(), EncodeError> {
        lock(&self.events).push(EncoderEvent::Finished(timeout));
        if self.finish_fails {
            return Err(EncodeError::TimedOut(timeout));
        }
        Ok(())
    }

    fn abort(self: Box<Self>) {
        lock(&self.events).push(EncoderEvent::Aborted);
    }
}

/// Один вызов сжатия JPEG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressCall {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub quality: u8,
}

/// Сжатие JPEG: выдаёт узнаваемое тело `FF D8 'J' 'P' 'G' FF D9`.
#[derive(Default)]
pub struct FakeJpeg {
    calls: Mutex<Vec<CompressCall>>,
    fail: AtomicBool,
}

impl FakeJpeg {
    pub const BODY: [u8; 7] = [0xFF, 0xD8, b'J', b'P', b'G', 0xFF, 0xD9];

    pub fn fail(&self) {
        self.fail.store(true, Ordering::SeqCst);
    }

    pub fn calls(&self) -> Vec<CompressCall> {
        lock(&self.calls).clone()
    }
}

impl JpegCompressor for FakeJpeg {
    fn compress(&self, rgba: &[u8], width: u32, height: u32, quality: u8) -> Result<Vec<u8>, EncodeError> {
        lock(&self.calls).push(CompressCall {
            rgba: rgba.to_vec(),
            width,
            height,
            quality,
        });
        if self.fail.load(Ordering::SeqCst) {
            return Err(EncodeError::Platform("Bitmap.compress вернул false".to_owned()));
        }
        Ok(Self::BODY.to_vec())
    }
}

/// Запись форматов-подделка: узнаваемые, но простые байты.
#[derive(Default)]
pub struct FakeFormats {
    fail_dng: AtomicBool,
    fail_exif: AtomicBool,
    exif_carriers: Mutex<Vec<RotationCarrier>>,
    dng_models: Mutex<Vec<String>>,
}

impl FakeFormats {
    /// Начало «DNG» подделки: как у настоящего (`II*\0`) плюс метка.
    pub const DNG_BYTES: &'static [u8] = b"II*\0FAKE-DNG";

    /// Сборка блока Exif начнёт отказывать.
    pub fn fail_exif(&self) {
        self.fail_exif.store(true, Ordering::SeqCst);
    }

    /// Где жил поворот в каждом запросе блока Exif.
    pub fn exif_carriers(&self) -> Vec<RotationCarrier> {
        lock(&self.exif_carriers).clone()
    }

    pub fn fail_dng(&self) {
        self.fail_dng.store(true, Ordering::SeqCst);
    }

    /// Модели камер, с которыми звали `write_dng`.
    pub fn dng_models(&self) -> Vec<String> {
        lock(&self.dng_models).clone()
    }

    /// Блок Exif, который строит подделка для картинки такого размера и модели.
    pub fn exif_for(width: u32, height: u32, model: &str) -> Vec<u8> {
        format!("EXIF:{width}x{height}:{model}").into_bytes()
    }
}

impl StillFileFormats for FakeFormats {
    fn write_dng(&self, inputs: &DngInputs<'_>, out: &mut dyn Write) -> Result<(), DngError> {
        lock(&self.dng_models).push(inputs.camera.model.clone());
        if self.fail_dng.load(Ordering::SeqCst) {
            return Err(DngError::InvalidInput("тест: кадр не годится".to_owned()));
        }
        out.write_all(Self::DNG_BYTES)?;
        Ok(())
    }

    fn build_exif(&self, inputs: &ExifInputs<'_>) -> Result<Vec<u8>, ExifError> {
        lock(&self.exif_carriers).push(inputs.rotation_carrier);
        if self.fail_exif.load(Ordering::SeqCst) {
            return Err(ExifError::Layout(TiffError::TooLarge));
        }
        Ok(Self::exif_for(
            inputs.pixel_size.width,
            inputs.pixel_size.height,
            &inputs.camera.model,
        ))
    }

    fn insert_jpeg_exif(&self, jpeg: &[u8], exif_tiff: &[u8]) -> Result<Vec<u8>, JpegExifError> {
        let Some(rest) = jpeg.strip_prefix(&[0xFF, 0xD8]) else {
            return Err(JpegExifError::NotJpeg("нет SOI".to_owned()));
        };
        let mut result = vec![0xFF, 0xD8];
        result.extend_from_slice(b"APP1[");
        result.extend_from_slice(exif_tiff);
        result.push(b']');
        result.extend_from_slice(rest);
        Ok(result)
    }
}

/// Часы: всегда 2026-11-22 10:15:30.123 UTC, смещение 0.
pub struct FixedClock;

impl FixedClock {
    pub const STEM: &'static str = "IMG_20261122_101530_123";
    pub const UTC_MILLIS: i64 = 1_795_342_530_123;
}

impl WallClock for FixedClock {
    fn now(&self) -> CaptureMoment {
        CaptureMoment {
            utc_millis: Self::UTC_MILLIS,
            utc_offset_minutes: 0,
        }
    }
}

/// Каталог: знает любую камеру, пока его не сделали «пустым».
#[derive(Default)]
pub struct FixedCameras {
    unknown: AtomicBool,
}

impl FixedCameras {
    pub const MODEL: &'static str = "PHB110";

    pub fn forget_all_cameras(&self) {
        self.unknown.store(true, Ordering::SeqCst);
    }
}

impl CameraCatalog for FixedCameras {
    fn describe_camera(&self, _camera: &CameraId) -> Option<CameraDescription> {
        if self.unknown.load(Ordering::SeqCst) {
            return None;
        }
        Some(CameraDescription {
            make: "OnePlus".to_owned(),
            model: Self::MODEL.to_owned(),
            unique_camera_model: "OnePlus PHB110 main".to_owned(),
            software: "Fasticamera test".to_owned(),
            lens: None,
        })
    }
}
