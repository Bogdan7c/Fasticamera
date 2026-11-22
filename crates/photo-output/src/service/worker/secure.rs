//! Захват кадра: `Secured` → черновики, кодер, цель для рендера, запись DNG.

use std::time::Instant;

use shell_contract::still_media::{HeifEncodeSettings, PendingMediaRequest};
use still_contract::develop::{PrimaryTarget, PrimaryTargetOffer};
use still_contract::phase::{StillFailure, StillPhase};
use still_contract::plan::{PrimaryFormat, RawOutput};
use still_contract::shot::{RawSamples16, StillFrameFacts};

use super::Worker;
use super::job::{Developing, Draft, Job, Stage};
use crate::dng::DngInputs;
use crate::exif::{CaptureMoment, ExifInputs, RotationCarrier};
use crate::service::media_writer::PendingMediaWriter;
use crate::service::naming::{DNG_MIME_TYPE, StillFileNames};

impl Worker {
    /// Кадр захвачен. Порядок важен: сначала цель уходит рендеру (он начинает проявлять), и только
    /// потом пишется тяжёлый DNG — пока GPU работает, процессор пишет файл.
    pub(super) fn on_secured(&mut self, facts: Box<StillFrameFacts>, raw: Option<RawSamples16>) {
        let id = facts.id;
        let Some(Job { ticket, stage }) = self.take_job(id) else {
            // Снимок уже сорван (срок, остановка): рендер ждёт ответа — отпускаем его.
            self.counters.record_late_event();
            self.offer_unavailable(id);
            return;
        };
        if matches!(stage, Stage::Developing(_)) {
            // Повторный Secured: первый уже обработан, рендеру ответ ушёл.
            self.counters.record_late_event();
            self.put_job(id, Job { ticket, stage });
            return;
        }
        self.deps.phases.report_phase(StillPhase::Secured { id });

        let (developing, target) = match self.open_resources(facts, raw.as_ref()) {
            Ok(opened) => opened,
            Err(failure) => {
                self.offer_unavailable(id);
                self.conclude(id, ticket, Err(failure));
                return;
            }
        };
        let mut developing = Box::new(developing);

        let delivered = self
            .deps
            .targets
            .offer_primary_target(PrimaryTargetOffer::Ready { id, target });
        let outcome = match delivered {
            Ok(()) => self.write_raw_draft(&mut developing, raw.as_ref()),
            Err(unavailable) => {
                tracing::warn!(?id, %unavailable, "рендер не принял цель для снимка");
                Err(StillFailure::ServiceUnavailable)
            }
        };
        let stage = Stage::Developing(developing);
        match outcome {
            Ok(()) => {
                self.put_job(id, Job { ticket, stage });
            }
            Err(failure) => self.fail_parts(id, ticket, stage, failure),
        }
    }

    /// Открывает черновики и (для HEIF) кодер. При любой ошибке всё уже открытое закрыто.
    fn open_resources(
        &self,
        facts: Box<StillFrameFacts>,
        raw: Option<&RawSamples16>,
    ) -> Result<(Developing, PrimaryTarget), StillFailure> {
        let camera_id = &facts.stream.camera;
        let camera = self
            .deps
            .cameras
            .describe_camera(camera_id)
            .ok_or_else(|| StillFailure::Store(format!("описание камеры {camera_id} недоступно")))?;
        if facts.outputs.needs_raw_samples() && raw.is_none() {
            return Err(StillFailure::Develop(
                "DNG в плане, а сырых данных рендер не прислал".to_owned(),
            ));
        }
        let moment = self.deps.clock.now();
        let names = StillFileNames::for_moment(&moment, facts.outputs.primary);
        let primary = self.create_draft(&names.primary, facts.outputs.primary.mime_type(), &moment)?;
        let mut developing = Developing {
            deadline: Instant::now() + self.config.develop_timeout(),
            facts,
            camera,
            moment,
            primary,
            raw: None,
            encoder: None,
        };
        match self.attach_remaining_resources(&mut developing, &names) {
            Ok(target) => Ok((developing, target)),
            Err(failure) => {
                developing.release_everything(&self.counters);
                Err(failure)
            }
        }
    }

    /// Черновик DNG (если в плане) и цель основной картинки.
    fn attach_remaining_resources(
        &self,
        developing: &mut Developing,
        names: &StillFileNames,
    ) -> Result<PrimaryTarget, StillFailure> {
        let moment = developing.moment;
        if developing.facts.outputs.raw == RawOutput::Dng {
            developing.raw = Some(self.create_draft(&names.dng, DNG_MIME_TYPE, &moment)?);
        }
        match developing.facts.outputs.primary {
            PrimaryFormat::Jpeg => Ok(PrimaryTarget::Readback),
            PrimaryFormat::Heif => self.open_heif_encoder(developing),
        }
    }

    fn open_heif_encoder(&self, developing: &mut Developing) -> Result<PrimaryTarget, StillFailure> {
        let facts = &developing.facts;
        let size = facts.primary_size();
        // HEIF: поворот несёт `irot` контейнера (`rotation_degrees` ниже), в Exif он не дублируется.
        let exif_tiff = self.build_exif_or_skip(&ExifInputs {
            rotation_carrier: RotationCarrier::ContainerTransform,
            facts,
            camera: &developing.camera,
            captured_at: &developing.moment,
            pixel_size: size,
        });
        let settings = HeifEncodeSettings {
            width: size.width,
            height: size.height,
            rotation_degrees: facts.rotation.degrees(),
            quality: self.config.heif_quality,
            exif_tiff,
        };
        let mut encoder = self
            .deps
            .heif
            .open(developing.primary.as_ref(), &settings)
            .map_err(|error| StillFailure::Encode(error.to_string()))?;
        match encoder.take_input_surface() {
            Some(surface) => {
                developing.encoder = Some(encoder);
                Ok(PrimaryTarget::EncoderSurface(surface))
            }
            None => {
                encoder.abort();
                Err(StillFailure::Encode(
                    "кодер HEIF не отдал входную поверхность".to_owned(),
                ))
            }
        }
    }

    fn create_draft(&self, display_name: &str, mime_type: &str, moment: &CaptureMoment) -> Result<Draft, StillFailure> {
        let request = PendingMediaRequest {
            display_name: display_name.to_owned(),
            mime_type: mime_type.to_owned(),
            relative_path: self.config.media_directory.clone(),
            date_taken_millis: moment.utc_millis,
        };
        self.deps
            .store
            .create_pending(&request)
            .map_err(|error| StillFailure::Store(error.to_string()))
    }

    /// Пишет DNG в его черновик (если DNG в плане).
    fn write_raw_draft(&self, developing: &mut Developing, raw: Option<&RawSamples16>) -> Result<(), StillFailure> {
        let (Some(draft), Some(raw)) = (developing.raw.as_mut(), raw) else {
            return Ok(());
        };
        let inputs = DngInputs {
            facts: &developing.facts,
            raw,
            camera: &developing.camera,
            captured_at: &developing.moment,
        };
        let mut writer = PendingMediaWriter::new(draft.as_mut());
        self.deps
            .formats
            .write_dng(&inputs, &mut writer)
            .map_err(|error| StillFailure::Store(format!("DNG: {error}")))
    }
}
