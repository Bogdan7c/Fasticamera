//! `AImageReader`, из которого кадры уходят в лизинг потока (`RawFrameStream`), и его безопасное
//! закрытие.
//!
//! HAL выбирает режим сенсора по RAW-потоку, поэтому и сессия, и стрим-режим 60 к/с держатся на
//! этом reader-е. Usage — `GPU_SAMPLED_IMAGE` (факт 00 C4): `GPU_DATA_BUFFER` для RAW gralloc не
//! выделяет, и камера молча отдаёт 0 кадров. `CPU_READ_RARELY` добавляется только когда включена
//! отладочная проба CPU-доступа (`cpu_access_probe`).
//!
//! # Жизненный цикл reader-а (главный риск — чтение освобождённой памяти)
//! `AImageReader_delete` освобождает все выданные образы, а кадр, выданный потоку, GPU может ещё
//! читать. Поэтому reader закрытой сессии проходит три шага:
//! 1. **Остановка** ([`RawReader::stop_offering`]) при закрытии сессии: поток отпускает свои кадры
//!    (`end_stream`), ворота слушателя закрываются — новых кадров из этого reader-а не будет.
//! 2. **Отсрочка** ([`RetiredReaders`]) при освобождении окна (следующая сессия или закрытие
//!    устройства): reader ждёт, пока потребители вернут свои кадры.
//! 3. **Удаление** (`AImageReader_delete`) на потоке камеры, когда невозвращённых кадров ноль.
//!    Если за отведённое время кадры не вернулись (остановка бэкенда) — reader утекает со счётчиком.

use std::ptr::{self, NonNull};
use std::sync::Arc;

use camera_contract::frame_passport::StreamPassport;
use camera_contract::geometry::PixelSize;
use camera_contract::raw_format::RawFormat;
use camera_core::backend::BackendFailure;
use camera_ndk_sys as ndk;
use raw_frame_stream::{RawFrameProducer, StreamEpoch};

use crate::cpu_access_probe::CpuAccessProbe;
use crate::diagnostics::BackendDiagnostics;
use crate::retired_readers::{RetirableReader, RetiredReaders};
use crate::status_mapping::media_backend_failure;
use crate::tags;

use super::reader_callback::{ReaderContext, reader_on_image_available};

/// Список отложенных reader-ов бэкенда.
pub(super) type RetiredNdkReaders = RetiredReaders<NdkReader>;

/// Сам `AImageReader` с контекстом слушателя: то, что откладывается и удаляется.
pub(super) struct NdkReader {
    reader: NonNull<ndk::AImageReader>,
    context: Arc<ReaderContext>,
}

// SAFETY: указатель `AImageReader` используется только для `AImageReader_delete` (на потоке камеры,
// куда объект и попадает), а потокобезопасность контекста слушателя обеспечивают его ворота и
// атомарные поля. Сам reader — объект NDK с внутренним замком.
unsafe impl Send for NdkReader {}

impl RetirableReader for NdkReader {
    fn outstanding_frames(&self) -> u64 {
        self.context.outstanding_frames()
    }

    fn delete(self) {
        // Идемпотентно: обычно остановка уже сделана при закрытии сессии, но путь ошибки создания
        // сюда приходит без неё. Ворота закрываем ДО delete (иначе взаимоблокировка с колбэком).
        self.context.stop();
        // SAFETY: reader создан `AImageReader_newWithUsage` и принадлежит только нам; ворота закрыты, ни
        // один колбэк не внутри NDK; невозвращённых кадров нет (проверено вызывающим), поэтому
        // освобождать нечего, на что мог бы смотреть GPU; камера отсоединена от окна (порядок
        // закрытия: сессия → устройство/следующая сессия → reader). Деструктор NDK дожидается
        // остановки looper-а, после чего контекст освобождается вместе с `self`.
        unsafe { ndk::AImageReader_delete(self.reader.as_ptr()) };
    }
}

/// Что нужно для создания reader-а.
pub(super) struct ReaderSpec {
    pub size: PixelSize,
    pub format: RawFormat,
    /// Постоянная часть паспорта сессии: уходит в `begin_stream`.
    pub passport: Arc<StreamPassport>,
    pub producer: RawFrameProducer,
    pub probe: CpuAccessProbe,
    pub diagnostics: Arc<BackendDiagnostics>,
    pub retirement: Arc<RetiredNdkReaders>,
}

/// Владелец reader-а сессии. При разрушении не удаляет reader, а останавливает и откладывает его.
pub(super) struct RawReader {
    /// `None` только после передачи в отсрочку (в `Drop`).
    reader: Option<NdkReader>,
    epoch: StreamEpoch,
    window: NonNull<ndk::ANativeWindow>,
    retirement: Arc<RetiredNdkReaders>,
}

impl RawReader {
    /// Начинает поток (`begin_stream`) и создаёт reader с `maxImages` из плана пула.
    ///
    /// Эпоха потока принадлежит этому reader-у: на любой ошибке поток закрывается (`end_stream`).
    pub(super) fn create(spec: ReaderSpec) -> Result<Self, BackendFailure> {
        let ReaderSpec {
            size,
            format,
            passport,
            producer,
            probe,
            diagnostics,
            retirement,
        } = spec;
        let start = producer.begin_stream(passport);
        let context = Arc::new(ReaderContext::new(
            diagnostics,
            producer,
            start.epoch,
            probe.clone(),
            format,
        ));
        // Любая ошибка ниже: поток закрываем, иначе эпоха осталась бы «живой» без reader-а.
        let fail = |failure: BackendFailure| {
            context.stop();
            failure
        };

        let invalid = |what: &str| BackendFailure {
            operation: "AImageReader_newWithUsage",
            code: 0,
            detail: format!("{what} не помещается в int32"),
        };
        let width = i32::try_from(size.width).map_err(|_| fail(invalid("ширина")))?;
        let height = i32::try_from(size.height).map_err(|_| fail(invalid("высота")))?;
        let max_images = i32::try_from(start.pool.reader_max_images()).map_err(|_| fail(invalid("maxImages")))?;
        let format_code = match format {
            RawFormat::Raw10 => tags::IMAGE_FORMAT_RAW10,
            RawFormat::Raw16 => tags::IMAGE_FORMAT_RAW16,
        };
        let usage = if probe.is_armed() {
            tags::USAGE_GPU_SAMPLED_IMAGE | tags::USAGE_CPU_READ_RARELY
        } else {
            tags::USAGE_GPU_SAMPLED_IMAGE
        };

        let mut reader: *mut ndk::AImageReader = ptr::null_mut();
        // SAFETY: аргументы — обычные числа, `reader` — валидное выходное место.
        let status =
            unsafe { ndk::AImageReader_newWithUsage(width, height, format_code, usage, max_images, &mut reader) };
        let Some(reader) = NonNull::new(reader).filter(|_| status == tags::MEDIA_STATUS_OK) else {
            return Err(fail(media_backend_failure("AImageReader_newWithUsage", status)));
        };
        // С этого места reader принадлежит `NdkReader`: при любой ошибке ниже его удалит `delete`.
        let ndk_reader = NdkReader {
            reader,
            context: Arc::clone(&context),
        };

        let mut window: *mut ndk::ANativeWindow = ptr::null_mut();
        // SAFETY: reader жив, `window` — валидное выходное место; окно принадлежит reader-у.
        let status = unsafe { ndk::AImageReader_getWindow(reader.as_ptr(), &mut window) };
        let Some(window) = NonNull::new(window).filter(|_| status == tags::MEDIA_STATUS_OK) else {
            // Слушатель ещё не ставился, кадров нет: удалять можно сразу.
            ndk_reader.delete();
            return Err(media_backend_failure("AImageReader_getWindow", status));
        };

        let mut listener = ndk::AImageReader_ImageListener {
            context: context.as_callback_context(),
            onImageAvailable: Some(reader_on_image_available),
        };
        // SAFETY: reader жив; NDK копирует структуру слушателя; контекст живёт до `AImageReader_delete`.
        let status = unsafe { ndk::AImageReader_setImageListener(reader.as_ptr(), &mut listener) };
        if status != tags::MEDIA_STATUS_OK {
            // Слушатель не установлен — колбэков не будет, кадров нет.
            ndk_reader.delete();
            return Err(media_backend_failure("AImageReader_setImageListener", status));
        }
        Ok(Self {
            reader: Some(ndk_reader),
            epoch: start.epoch,
            window,
            retirement,
        })
    }

    /// Окно, в которое камера пишет кадры (принадлежит reader-у и живёт столько же).
    pub(super) fn window(&self) -> *mut ndk::ANativeWindow {
        self.window.as_ptr()
    }

    /// Эпоха потока этого reader-а (для результатов запросов этой же сессии).
    pub(super) fn epoch(&self) -> StreamEpoch {
        self.epoch
    }

    /// Закрытие сессии: кадры из этого reader-а больше не предлагаются потоку. Идемпотентно.
    /// Reader при этом остаётся жить (окно ещё привязано к устройству).
    pub(super) fn stop_offering(&self) {
        if let Some(reader) = &self.reader {
            reader.context.stop();
        }
    }
}

impl Drop for RawReader {
    fn drop(&mut self) {
        // Окно освобождено (вызывающий дошёл до этого шага порядка закрытия): reader не удаляем сразу,
        // а откладываем до возврата всех кадров.
        if let Some(reader) = self.reader.take() {
            reader.context.stop();
            self.retirement.retire(reader);
        }
    }
}
