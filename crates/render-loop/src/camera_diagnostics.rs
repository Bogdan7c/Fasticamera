//! Диагностика показа кадров камеры: нейтральные структуры для статуса потока рендера.
//!
//! Счётчики копятся в бэкенде и приходят в [`crate::thread::RenderStatus`] через
//! [`crate::backend::BackendDiagnostics`]. Часы НЕ смешиваются: метка сенсора — шкала `BOOTTIME`,
//! момент показа — `MONOTONIC`; перевод и расчёт задержки «сенсор -> дисплей» делает приложение,
//! здесь хранится только пара «id показа — метка сенсора» (по `present_id` её сопоставляют с
//! [`crate::backend::PastPresentation`]).

/// Какой кадр камеры показан последним.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentedCameraFrame {
    /// Порядковый номер кадра в нашем потоке (`FrameSequence`).
    pub sequence: u64,
    /// Метка сенсора кадра, наносекунды шкалы `BOOTTIME`.
    pub sensor_timestamp_nanos: i64,
    /// Номер кадра Camera2, если у кадра есть пара-результат (`None` — метаданных нет).
    pub frame_number: Option<i64>,
    /// Идентификатор показа (`present_id`, циклический `u32`).
    pub present_id: u32,
}

/// Запись истории показов: чей кадр ушёл в показ под этим `present_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentedSample {
    /// Идентификатор показа.
    pub present_id: u32,
    /// Метка сенсора показанного кадра, наносекунды `BOOTTIME`.
    pub sensor_timestamp_nanos: i64,
}

/// Счётчики показа кадров камеры. Все поля накопительные за жизнь бэкенда, кроме истории.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CameraStreamDiagnostics {
    /// Сколько РАЗНЫХ кадров камеры поставлено на показ (первый показ каждого).
    pub camera_frames_presented: u64,
    /// Сколько раз уже показанный кадр нарисован снова (новых кадров не было).
    pub camera_frame_redraws: u64,
    /// Сколько тиков показа пропущено нарочно, потому что на экране ничего бы не изменилось (решение 214): зритель
    /// снова видит уже показанный кадр, но кадр не рисовался и не показывался. Вместе с `camera_frame_redraws` даёт
    /// «видимые повторы» ([`Self::visible_repeats`]): метрика не должна зависеть от того, рисуется ли повтор.
    pub camera_frame_repeats_held: u64,
    /// Сколько кадров импортировано в GPU (новых лизингов, дошедших до рисования).
    pub camera_frames_imported: u64,
    /// Сколько новых кадров не удалось подготовить (импорт, fence, паспорт): кадр возвращён нетронутым.
    pub camera_frame_failures: u64,
    /// Попадания кэша импортов.
    pub import_cache_hits: u64,
    /// Промахи кэша импортов (настоящие импорты).
    pub import_cache_misses: u64,
    /// Сколько release-fence экспортировано и отдано камере.
    pub release_fences_exported: u64,
    /// Сколько экспортов release-fence не удалось.
    pub release_export_failures: u64,
    /// Сколько раз не удалось создать новый семафор экспорта после неудачного экспорта (повторы идут с
    /// паузой в кадрах, GPU не ждётся).
    pub export_semaphore_recreate_failures: u64,
    /// Сколько лизингов возвращено камере по завершению серии сабмитов (без fence): экспорт был недоступен.
    pub leases_released_by_completion: u64,
    /// Сколько лизингов возвращено камере после подтверждённого простоя GPU при отсоединении (без fence).
    pub leases_released_after_idle: u64,
    /// Сколько лизингов намеренно выброшено при зависшем GPU (учтённая утечка, решение 43).
    pub leases_leaked_on_teardown: u64,
    /// Последний показанный кадр.
    pub last_presented_frame: Option<PresentedCameraFrame>,
    /// Короткая история показов (старые — первыми), для расчёта задержки «сенсор -> дисплей».
    pub present_history: Vec<PresentedSample>,
}

impl CameraStreamDiagnostics {
    /// Сколько раз зритель видел уже показанный кадр снова: повторы, которые нарисованы, и повторы, которые
    /// пропущены как «ничего не изменилось». Не зависит от пропуска тиков, поэтому профили подачи сравнимы.
    pub fn visible_repeats(&self) -> u64 {
        self.camera_frame_redraws + self.camera_frame_repeats_held
    }

    /// Записывает первый показ нового кадра, держа историю в пределах `history_capacity`.
    pub(crate) fn note_new_frame_presented(&mut self, frame: PresentedCameraFrame, history_capacity: usize) {
        self.camera_frames_presented += 1;
        self.last_presented_frame = Some(frame);
        self.present_history.push(PresentedSample {
            present_id: frame.present_id,
            sensor_timestamp_nanos: frame.sensor_timestamp_nanos,
        });
        if self.present_history.len() > history_capacity {
            let excess = self.present_history.len() - history_capacity;
            self.present_history.drain(..excess);
        }
    }

    /// Прибавляет итог закончившейся сессии показа (счётчики складываются, «последний кадр» берётся из неё,
    /// если он у неё есть, история дописывается).
    pub(crate) fn absorb_finished_session(&mut self, finished: &Self, history_capacity: usize) {
        self.camera_frames_presented += finished.camera_frames_presented;
        self.camera_frame_redraws += finished.camera_frame_redraws;
        self.camera_frame_repeats_held += finished.camera_frame_repeats_held;
        self.camera_frames_imported += finished.camera_frames_imported;
        self.camera_frame_failures += finished.camera_frame_failures;
        self.import_cache_hits += finished.import_cache_hits;
        self.import_cache_misses += finished.import_cache_misses;
        self.release_fences_exported += finished.release_fences_exported;
        self.release_export_failures += finished.release_export_failures;
        self.export_semaphore_recreate_failures += finished.export_semaphore_recreate_failures;
        self.leases_released_by_completion += finished.leases_released_by_completion;
        self.leases_released_after_idle += finished.leases_released_after_idle;
        self.leases_leaked_on_teardown += finished.leases_leaked_on_teardown;
        if finished.last_presented_frame.is_some() {
            self.last_presented_frame = finished.last_presented_frame;
        }
        self.present_history.extend_from_slice(&finished.present_history);
        if self.present_history.len() > history_capacity {
            let excess = self.present_history.len() - history_capacity;
            self.present_history.drain(..excess);
        }
    }

    /// Сумма двух снимков для отчёта: накопленное прошлыми сессиями плюс текущая.
    pub(crate) fn combined_with(&self, live: &Self, history_capacity: usize) -> Self {
        let mut total = self.clone();
        total.absorb_finished_session(live, history_capacity);
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(sequence: u64) -> PresentedCameraFrame {
        PresentedCameraFrame {
            sequence,
            sensor_timestamp_nanos: 1_000 * i64::try_from(sequence).unwrap_or(0),
            frame_number: Some(i64::try_from(sequence).unwrap_or(0) + 100),
            present_id: u32::try_from(sequence).unwrap_or(0),
        }
    }

    #[test]
    fn history_keeps_only_the_newest_entries_in_order() {
        let mut diagnostics = CameraStreamDiagnostics::default();
        for sequence in 0..5 {
            diagnostics.note_new_frame_presented(frame(sequence), 3);
        }
        assert_eq!(diagnostics.camera_frames_presented, 5);
        let ids: Vec<u32> = diagnostics
            .present_history
            .iter()
            .map(|sample| sample.present_id)
            .collect();
        assert_eq!(ids, vec![2, 3, 4]);
        assert_eq!(diagnostics.last_presented_frame, Some(frame(4)));
    }

    #[test]
    fn finished_session_adds_up_and_keeps_the_latest_known_frame() {
        let mut totals = CameraStreamDiagnostics::default();
        totals.note_new_frame_presented(frame(1), 8);
        totals.release_fences_exported = 4;
        let mut session = CameraStreamDiagnostics::default();
        session.note_new_frame_presented(frame(7), 8);
        session.release_fences_exported = 2;
        session.release_export_failures = 1;
        let combined = totals.combined_with(&session, 8);
        assert_eq!(combined.camera_frames_presented, 2);
        assert_eq!(
            (combined.release_fences_exported, combined.release_export_failures),
            (6, 1)
        );
        assert_eq!(combined.last_presented_frame, Some(frame(7)));
        assert_eq!(combined.present_history.len(), 2);
        assert_eq!(totals.camera_frames_presented, 1, "исходный снимок не меняется");

        let silent = CameraStreamDiagnostics::default();
        assert_eq!(
            combined.combined_with(&silent, 8).last_presented_frame,
            Some(frame(7)),
            "сессия без кадров не стирает последний показанный"
        );
    }
}
