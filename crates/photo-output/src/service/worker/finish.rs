//! Завершение снимка: итоговые события рендера → кодер/JPEG → публикация «всё или ничего».
//!
//! Порядок публикации: СНАЧАЛА основной файл, потом DNG. Публикация необратима (трейт не умеет
//! удалять опубликованное), поэтому порядок выбран так, чтобы частичный итог был невозможен там,
//! где это важнее всего:
//! * основной не опубликовался → DNG ещё черновик, его удаляем: в галерее ничего нет;
//! * основной опубликован, DNG — нет: снимок в галерее уже есть, его не откатить. Это единственное
//!   «полу-состояние»; решение главного агента: итог `Saved{primary, raw: None}` (экран покажет
//!   миниатюру, а не ошибку), а потеря DNG не молчит — счётчик `raw_lost_after_primary_published`
//!   и лог `error` с причиной. Платформенная публикация — смена одного флага `IS_PENDING`,
//!   поэтому отказ второго вызова после успешного первого практически исключён.

use shell_contract::still_media::PendingMedia;
use still_contract::phase::{SavedStill, StillFailure};
use still_contract::plan::{PrimaryFormat, StillId};
use still_contract::shot::Rgba8Image;

use super::Worker;
use super::job::{Developing, Draft, Job, discard_draft};
use crate::exif::{CameraDescription, CaptureMoment, ExifInputs, RotationCarrier};
use still_contract::shot::StillFrameFacts;

impl Worker {
    /// Рендер показал кадр в поверхность кодера HEIF.
    pub(super) fn on_primary_rendered(&mut self, id: StillId) {
        let Some((ticket, developing)) = self.take_developing(id, PrimaryFormat::Heif) else {
            return;
        };
        let outcome = self.complete_heif(*developing);
        self.conclude(id, ticket, outcome);
    }

    /// Рендер прочитал картинку для JPEG.
    pub(super) fn on_primary_readback(&mut self, id: StillId, image: &Rgba8Image) {
        let Some((ticket, developing)) = self.take_developing(id, PrimaryFormat::Jpeg) else {
            return;
        };
        let outcome = self.complete_jpeg(*developing, image);
        self.conclude(id, ticket, outcome);
    }

    /// Достаёт снимок в состоянии «проявляется» с ожидаемым форматом. Опоздавшее событие считается
    /// и игнорируется; событие, не подходящее состоянию (протокол нарушен), срывает снимок.
    fn take_developing(
        &mut self,
        id: StillId,
        expected: PrimaryFormat,
    ) -> Option<(super::AdmissionTicket, Box<Developing>)> {
        let Job { ticket, stage } = self.take_job_or_count_late(id)?;
        match stage {
            super::Stage::Developing(developing) if developing.facts.outputs.primary == expected => {
                Some((ticket, developing))
            }
            unexpected => {
                let failure = StillFailure::Develop("итоговое событие не соответствует состоянию снимка".to_owned());
                self.fail_parts(id, ticket, unexpected, failure);
                None
            }
        }
    }

    fn complete_heif(&self, developing: Developing) -> Result<SavedStill, StillFailure> {
        let Developing {
            encoder, primary, raw, ..
        } = developing;
        let Some(encoder) = encoder else {
            self.discard_both(primary, raw);
            return Err(StillFailure::Encode("кодер HEIF не открыт".to_owned()));
        };
        if let Err(error) = encoder.finish(self.config.heif_finish_timeout()) {
            // Файл после неудачного `finish` не годен.
            self.discard_both(primary, raw);
            return Err(StillFailure::Encode(error.to_string()));
        }
        self.publish_all(primary, raw)
    }

    fn complete_jpeg(&self, developing: Developing, image: &Rgba8Image) -> Result<SavedStill, StillFailure> {
        let Developing {
            facts,
            camera,
            moment,
            mut primary,
            raw,
            ..
        } = developing;
        match self.write_jpeg(&facts, &camera, &moment, primary.as_mut(), image) {
            Ok(()) => self.publish_all(primary, raw),
            Err(failure) => {
                self.discard_both(primary, raw);
                Err(failure)
            }
        }
    }

    /// Сжимает картинку, вставляет Exif и дописывает файл в черновик.
    fn write_jpeg(
        &self,
        facts: &StillFrameFacts,
        camera: &CameraDescription,
        moment: &CaptureMoment,
        primary: &mut dyn PendingMedia,
        image: &Rgba8Image,
    ) -> Result<(), StillFailure> {
        let size = image.size();
        let jpeg = self
            .deps
            .jpeg
            .compress(image.rgba_bytes(), size.width, size.height, self.config.jpeg_quality)
            .map_err(|error| StillFailure::Encode(error.to_string()))?;
        let exif_tiff = self.build_exif_or_skip(&ExifInputs {
            rotation_carrier: RotationCarrier::ExifOrientationTag,
            facts,
            camera,
            captured_at: moment,
            pixel_size: size,
        });
        // Без блока Exif (сбой сборки) JPEG пишется как есть: снимок важнее метаданных.
        let file_bytes = match exif_tiff {
            Some(exif_tiff) => self
                .deps
                .formats
                .insert_jpeg_exif(&jpeg, &exif_tiff)
                .map_err(|error| StillFailure::Encode(error.to_string()))?,
            None => jpeg,
        };
        primary
            .write_all(&file_bytes)
            .map_err(|error| StillFailure::Store(error.to_string()))
    }

    /// Публикует файлы снимка «всё или ничего» (порядок — в доке модуля).
    fn publish_all(&self, primary: Draft, raw: Option<Draft>) -> Result<SavedStill, StillFailure> {
        let primary_media = match primary.publish() {
            Ok(media) => media,
            Err(error) => {
                if let Some(raw) = raw {
                    discard_draft(&self.counters, raw);
                }
                return Err(StillFailure::Store(format!("публикация основного файла: {error}")));
            }
        };
        let Some(raw) = raw else {
            return Ok(SavedStill {
                primary: primary_media.uri,
                raw: None,
            });
        };
        match raw.publish() {
            Ok(raw_media) => Ok(SavedStill {
                primary: primary_media.uri,
                raw: Some(raw_media.uri),
            }),
            Err(error) => {
                self.counters.record_raw_lost_after_primary_published();
                tracing::error!(%error, primary = %primary_media.uri, "DNG не опубликован, основной файл уже в галерее");
                Ok(SavedStill {
                    primary: primary_media.uri,
                    raw: None,
                })
            }
        }
    }

    fn discard_both(&self, primary: Draft, raw: Option<Draft>) {
        discard_draft(&self.counters, primary);
        if let Some(raw) = raw {
            discard_draft(&self.counters, raw);
        }
    }
}
