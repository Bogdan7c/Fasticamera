//! Захват кадра: `Secured` → черновики, цель для рендера (кодер — по очереди мест), запись DNG.

use std::time::Instant;

use shell_contract::still_media::{HeifEncodeSettings, PendingMediaRequest};
use still_contract::develop::{PrimaryTarget, PrimaryTargetOffer};
use still_contract::phase::StillFailure;
use still_contract::plan::{PrimaryFormat, RawOutput, StillId};
use still_contract::shot::{RawSamples16, StillFrameFacts};

use super::Worker;
use super::encoder_queue::EncoderSeat;
use super::job::{Developing, Draft, Job, Stage};
use crate::dng::DngInputs;
use crate::exif::{CaptureMoment, ExifInputs, RotationCarrier};
use crate::service::media_writer::PendingMediaWriter;
use crate::service::naming::{DNG_MIME_TYPE, StillFileNames};

impl Worker {
    /// Кадр захвачен (`arrived_at` — момент прихода `Secured` в службу, от него идёт срок проявления).
    ///
    /// Фазу `Secured` экрану тут уже никто не шлёт: её отправил `StillServiceHandle` в момент приёма.
    /// Порядок: черновики → цель рендеру (JPEG — сразу, HEIF — если есть место под кодер, иначе снимок
    /// встаёт в очередь) → тяжёлый DNG. Пока GPU проявляет, процессор пишет файл.
    pub(super) fn on_secured(&mut self, facts: Box<StillFrameFacts>, raw: Option<RawSamples16>, arrived_at: Instant) {
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
        let developing = match self.open_drafts(facts, raw.as_ref(), arrived_at) {
            Ok(developing) => Box::new(developing),
            Err(failure) => {
                self.offer_unavailable(id);
                self.conclude(id, ticket, Err(failure));
                return;
            }
        };
        let is_heif = developing.facts.outputs.primary == PrimaryFormat::Heif;
        self.put_job(
            id,
            Job {
                ticket,
                stage: Stage::Developing(developing),
            },
        );
        if is_heif {
            self.encoder_queue.enqueue(id);
            self.open_waiting_encoders();
            self.count_deferred_if_still_waiting(id);
        } else if let Err(failure) = self.offer_readback_target(id) {
            self.fail_in_table(id, failure);
        }
        self.write_raw_after_target(id, raw.as_ref());
    }

    /// Снимок остался без кодера из-за нехватки мест: считаем (место появится позже).
    fn count_deferred_if_still_waiting(&self, id: StillId) {
        if let Some(Job {
            stage: Stage::Developing(developing),
            ..
        }) = self.jobs.get(&id)
            && developing.encoder.is_waiting()
        {
            self.counters.record_encoder_offer_deferred();
        }
    }

    /// Отдаёт рендеру цель JPEG (память, а не кодер).
    fn offer_readback_target(&self, id: StillId) -> Result<(), StillFailure> {
        let offer = PrimaryTargetOffer::Ready {
            id,
            target: PrimaryTarget::Readback,
        };
        self.deps.targets.offer_primary_target(offer).map_err(|unavailable| {
            tracing::warn!(?id, %unavailable, "рендер не принял цель для снимка");
            StillFailure::ServiceUnavailable
        })
    }

    /// Срывает снимок, который лежит в таблице (снимает его оттуда и фиксирует итог).
    fn fail_in_table(&mut self, id: StillId, failure: StillFailure) {
        if let Some(job) = self.take_job(id) {
            self.fail_job(id, job, failure);
        }
    }

    /// Пишет DNG снимка (если он есть в таблице и DNG в плане). Сбой записи срывает снимок целиком.
    fn write_raw_after_target(&mut self, id: StillId, raw: Option<&RawSamples16>) {
        // Снимок мог закончиться при выдаче цели (кодер не открылся, рендер остановлен).
        let Some(Job { ticket, stage }) = self.take_job(id) else {
            return;
        };
        let Stage::Developing(mut developing) = stage else {
            self.put_job(id, Job { ticket, stage });
            return;
        };
        match self.write_raw_draft(&mut developing, raw) {
            Ok(()) => self.put_job(
                id,
                Job {
                    ticket,
                    stage: Stage::Developing(developing),
                },
            ),
            Err(failure) => self.fail_parts(id, ticket, Stage::Developing(developing), failure),
        }
    }

    /// Открывает черновики основной картинки и DNG. Кодер здесь не открывается: его место выдаёт очередь.
    /// При любой ошибке всё уже открытое закрыто.
    fn open_drafts(
        &self,
        facts: Box<StillFrameFacts>,
        raw: Option<&RawSamples16>,
        arrived_at: Instant,
    ) -> Result<Developing, StillFailure> {
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
        let encoder = match facts.outputs.primary {
            PrimaryFormat::Jpeg => EncoderSeat::NotNeeded,
            PrimaryFormat::Heif => EncoderSeat::Waiting,
        };
        let mut developing = Developing {
            // Срок идёт от прихода `Secured`, а не от момента, когда поток дошёл до него.
            deadline: arrived_at + self.config.develop_timeout(),
            facts,
            camera,
            moment,
            primary,
            raw: None,
            encoder,
        };
        if developing.facts.outputs.raw == RawOutput::Dng {
            match self.create_draft(&names.dng, DNG_MIME_TYPE, &moment) {
                Ok(draft) => developing.raw = Some(draft),
                Err(failure) => {
                    developing.release_everything(&self.counters);
                    return Err(failure);
                }
            }
        }
        Ok(developing)
    }

    /// Открывает кодер HEIF снимка и возвращает его поверхность как цель рендера. Место под кодер
    /// занимается только при успехе.
    pub(super) fn open_heif_encoder(&self, developing: &mut Developing) -> Result<PrimaryTarget, StillFailure> {
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
                developing.encoder = self.seat_for_open_encoder(encoder);
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
