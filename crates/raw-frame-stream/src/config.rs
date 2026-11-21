//! Параметры потока кадров (секция конфига `camera.raw_stream`).

use serde::Deserialize;

use crate::viewfinder_pacing::{JitterBufferConfig, ViewfinderPacing};

/// Параметры лизинга. Значения по умолчанию обоснованы в отчёте сессии 05.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RawFrameStreamConfig {
    /// Сколько кадр ждёт результат запроса с той же меткой, прежде чем уйти с пометкой
    /// «метаданных нет» (решение 42).
    pub pairing_timeout_ms: u32,
    /// Сколько кадров одновременно ждут пару. Каждый держит буфер камеры — входит в план пула.
    /// Если пришёл ещё один, старейший уходит без метаданных (`PairingTimedOut`) досрочно.
    pub max_pending_frames: u32,
    /// Сколько результатов ждут свой кадр (результат может прийти раньше буфера).
    pub max_buffered_results: u32,
    /// Сколько кадров видоискателя держит поток рендера: показанный сейчас + только что взятый.
    pub viewfinder_render_holds: u32,
    /// Бюджет памяти кольца снимков `KeepLast(N)`, байты. 0 — кольцо выключено (решение 41).
    pub snapshot_ring_budget_bytes: u64,
    /// Потолок числа кадров кольца, даже если бюджет позволяет больше.
    pub snapshot_ring_max_frames: u32,
    /// Запас буферов сверх расчёта (колбэк reader-а держит кадр, пока раздаёт его).
    pub pool_slack: u32,
    /// Политика видоискателя на старте (решение 53: буфер с запасом). На ходу меняется
    /// `RawFrameStream::set_viewfinder_pacing` (A/B на телефоне).
    pub viewfinder_pacing: ViewfinderPacing,
    /// Параметры буфера видоискателя. План пула всегда учитывает его ячейки, даже при
    /// `LatestWins`: переключение на ходу не должно исчерпать `maxImages` reader-а.
    pub viewfinder_jitter_buffer: JitterBufferConfig,
    /// Сколько тёплых (standby) эпох может жить одновременно, кроме показываемой (сессия 13b).
    /// 0 — тёплые запрещены. Каждая держит целый reader с полным планом пула, поэтому предел мал.
    pub max_standby_streams: u32,
}

impl Default for RawFrameStreamConfig {
    fn default() -> Self {
        Self {
            pairing_timeout_ms: 50,
            max_pending_frames: 2,
            max_buffered_results: 8,
            viewfinder_render_holds: 2,
            snapshot_ring_budget_bytes: 0,
            snapshot_ring_max_frames: 30,
            pool_slack: 1,
            viewfinder_pacing: ViewfinderPacing::default(),
            viewfinder_jitter_buffer: JitterBufferConfig::default(),
            max_standby_streams: 1,
        }
    }
}

/// Недопустимые параметры потока.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RawFrameStreamConfigError {
    #[error("camera.raw_stream.{field} = {value}: допустимо {min}..={max}")]
    OutOfRange {
        field: &'static str,
        value: u64,
        min: u64,
        max: u64,
    },
    #[error(
        "camera.raw_stream.viewfinder_jitter_buffer: max_queued ({max_queued}) должен быть не меньше \
         reserve + 1 ({reserve} + 1), иначе буфер никогда не заполнится"
    )]
    JitterQueueTooShallow { reserve: u32, max_queued: u32 },
}

/// Границы параметров. Вынесены в константы, чтобы тесты и сообщения об ошибках не расходились.
mod bounds {
    use std::ops::RangeInclusive;

    /// Меньше 1 мс пару не поймать (колбэки кадра и результата идут разными потоками),
    /// больше секунды видоискатель отстаёт заметно.
    pub(super) const PAIRING_TIMEOUT_MS: RangeInclusive<u64> = 1..=1000;
    /// Хотя бы один кадр должен уметь ждать пару; больше 8 — перерасход буферов камеры.
    pub(super) const MAX_PENDING_FRAMES: RangeInclusive<u64> = 1..=8;
    pub(super) const MAX_BUFFERED_RESULTS: RangeInclusive<u64> = 1..=64;
    /// Рендер держит минимум один кадр; больше четырёх не бывает при двойной-тройной буферизации.
    pub(super) const VIEWFINDER_RENDER_HOLDS: RangeInclusive<u64> = 1..=4;
    /// Потолок кольца: 64 кадра 4096x3072 RAW10 уже около 1 ГиБ.
    pub(super) const SNAPSHOT_RING_MAX_FRAMES: RangeInclusive<u64> = 0..=64;
    pub(super) const POOL_SLACK: RangeInclusive<u64> = 0..=4;
    /// Запас больше 4 кадров — больше 66 мс задержки при 60 Гц: это уже не сглаживание дрожания.
    /// Тёплых больше четырёх — это уже второй показываемый поток по памяти (каждый reader ≈ 15 буферов).
    pub(super) const MAX_STANDBY_STREAMS: RangeInclusive<u64> = 0..=4;
    pub(super) const JITTER_RESERVE: RangeInclusive<u64> = 0..=4;
    /// Каждая ячейка — буфер камеры (RAW10 12 Мп ≈ 15 МиБ); больше 8 — перерасход памяти.
    pub(super) const JITTER_MAX_QUEUED: RangeInclusive<u64> = 1..=8;
}

impl RawFrameStreamConfig {
    /// Проверяет границы каждого параметра. Бюджет кольца ограничений не имеет:
    /// любое значение осмысленно (0 выключает кольцо).
    pub fn validate(&self) -> Result<(), RawFrameStreamConfigError> {
        check_range(
            "pairing_timeout_ms",
            self.pairing_timeout_ms,
            &bounds::PAIRING_TIMEOUT_MS,
        )?;
        check_range(
            "max_pending_frames",
            self.max_pending_frames,
            &bounds::MAX_PENDING_FRAMES,
        )?;
        check_range(
            "max_buffered_results",
            self.max_buffered_results,
            &bounds::MAX_BUFFERED_RESULTS,
        )?;
        check_range(
            "viewfinder_render_holds",
            self.viewfinder_render_holds,
            &bounds::VIEWFINDER_RENDER_HOLDS,
        )?;
        check_range(
            "snapshot_ring_max_frames",
            self.snapshot_ring_max_frames,
            &bounds::SNAPSHOT_RING_MAX_FRAMES,
        )?;
        check_range("pool_slack", self.pool_slack, &bounds::POOL_SLACK)?;
        check_range(
            "max_standby_streams",
            self.max_standby_streams,
            &bounds::MAX_STANDBY_STREAMS,
        )?;
        self.validate_jitter_buffer()
    }

    /// Сколько ячеек видоискателя входит в план пула: очередь буфера (или одна ячейка
    /// `LatestWins`, если очередь короче) + кадры у потока рендера. Считается по верхней границе
    /// обеих политик: политика переключается на ходу, а `maxImages` reader-а — на всю сессию.
    pub fn viewfinder_pool_cells(&self) -> u32 {
        self.viewfinder_jitter_buffer.max_queued.max(1) + self.viewfinder_render_holds
    }

    fn validate_jitter_buffer(&self) -> Result<(), RawFrameStreamConfigError> {
        let buffer = self.viewfinder_jitter_buffer;
        check_range(
            "viewfinder_jitter_buffer.reserve",
            buffer.reserve,
            &bounds::JITTER_RESERVE,
        )?;
        check_range(
            "viewfinder_jitter_buffer.max_queued",
            buffer.max_queued,
            &bounds::JITTER_MAX_QUEUED,
        )?;
        if buffer.max_queued < buffer.reserve + 1 {
            return Err(RawFrameStreamConfigError::JitterQueueTooShallow {
                reserve: buffer.reserve,
                max_queued: buffer.max_queued,
            });
        }
        Ok(())
    }
}

/// Одно поле в границах или ошибка с именем поля.
fn check_range(
    field: &'static str,
    value: u32,
    allowed: &std::ops::RangeInclusive<u64>,
) -> Result<(), RawFrameStreamConfigError> {
    let value = u64::from(value);
    if allowed.contains(&value) {
        return Ok(());
    }
    Err(RawFrameStreamConfigError::OutOfRange {
        field,
        value,
        min: *allowed.start(),
        max: *allowed.end(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        assert_eq!(RawFrameStreamConfig::default().validate(), Ok(()));
    }

    #[test]
    fn every_bounded_field_rejects_values_outside_its_range_and_names_itself() {
        type Mutate = fn(&mut RawFrameStreamConfig);
        let cases: [(&str, Mutate, u64, u64); 11] = [
            ("pairing_timeout_ms", |c| c.pairing_timeout_ms = 0, 1, 1000),
            ("pairing_timeout_ms", |c| c.pairing_timeout_ms = 1001, 1, 1000),
            ("max_pending_frames", |c| c.max_pending_frames = 0, 1, 8),
            ("max_pending_frames", |c| c.max_pending_frames = 9, 1, 8),
            ("max_buffered_results", |c| c.max_buffered_results = 0, 1, 64),
            ("viewfinder_render_holds", |c| c.viewfinder_render_holds = 0, 1, 4),
            ("viewfinder_render_holds", |c| c.viewfinder_render_holds = 5, 1, 4),
            ("snapshot_ring_max_frames", |c| c.snapshot_ring_max_frames = 65, 0, 64),
            ("pool_slack", |c| c.pool_slack = 5, 0, 4),
            ("max_buffered_results", |c| c.max_buffered_results = 65, 1, 64),
            ("max_standby_streams", |c| c.max_standby_streams = 5, 0, 4),
        ];
        for (field, mutate, min, max) in cases {
            let mut config = RawFrameStreamConfig::default();
            mutate(&mut config);
            let error = config.validate().expect_err("значение вне границ должно отвергаться");
            let RawFrameStreamConfigError::OutOfRange {
                field: reported,
                min: reported_min,
                max: reported_max,
                ..
            } = error
            else {
                panic!("ожидалась ошибка границ, а не {error:?}");
            };
            assert_eq!((reported, reported_min, reported_max), (field, min, max));
        }
    }

    #[test]
    fn boundary_values_and_any_budget_are_accepted() {
        let config = RawFrameStreamConfig {
            pairing_timeout_ms: 1,
            max_pending_frames: 8,
            max_buffered_results: 64,
            viewfinder_render_holds: 1,
            snapshot_ring_budget_bytes: u64::MAX,
            snapshot_ring_max_frames: 0,
            pool_slack: 0,
            viewfinder_pacing: ViewfinderPacing::LatestWins,
            viewfinder_jitter_buffer: JitterBufferConfig {
                reserve: 0,
                max_queued: 1,
            },
            max_standby_streams: 0,
        };
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn defaults_choose_the_jitter_buffer_with_one_reserve_frame() {
        let config = RawFrameStreamConfig::default();
        assert_eq!(config.viewfinder_pacing, ViewfinderPacing::JitterBuffer);
        assert_eq!(
            config.viewfinder_jitter_buffer,
            JitterBufferConfig {
                reserve: 1,
                max_queued: 2
            }
        );
        // Очередь 2 + рендер 2: ячейки считаются по верхней границе обеих политик.
        assert_eq!(config.viewfinder_pool_cells(), 4);
    }

    #[test]
    fn jitter_buffer_fields_are_bounded_and_the_queue_must_fit_the_reserve() {
        let with_buffer = |reserve, max_queued| RawFrameStreamConfig {
            viewfinder_jitter_buffer: JitterBufferConfig { reserve, max_queued },
            ..RawFrameStreamConfig::default()
        };
        assert!(matches!(
            with_buffer(5, 8).validate(),
            Err(RawFrameStreamConfigError::OutOfRange {
                field: "viewfinder_jitter_buffer.reserve",
                ..
            })
        ));
        assert!(matches!(
            with_buffer(1, 9).validate(),
            Err(RawFrameStreamConfigError::OutOfRange {
                field: "viewfinder_jitter_buffer.max_queued",
                ..
            })
        ));
        assert_eq!(
            with_buffer(2, 2).validate(),
            Err(RawFrameStreamConfigError::JitterQueueTooShallow {
                reserve: 2,
                max_queued: 2
            })
        );
        assert_eq!(with_buffer(2, 3).validate(), Ok(()));
    }

    #[test]
    fn pacing_is_read_by_name_and_unknown_buffer_keys_are_rejected() {
        let config: RawFrameStreamConfig = serde_json::from_str(
            r#"{"viewfinder_pacing": "latest_wins", "viewfinder_jitter_buffer": {"reserve": 2, "max_queued": 3}}"#,
        )
        .expect("json");
        assert_eq!(config.viewfinder_pacing, ViewfinderPacing::LatestWins);
        assert_eq!(config.viewfinder_jitter_buffer.max_queued, 3);
        // LatestWins с очередью 3: план пула всё равно держит 3 ячейки — на случай переключения.
        assert_eq!(config.viewfinder_pool_cells(), 3 + 2);
        assert!(serde_json::from_str::<RawFrameStreamConfig>(r#"{"viewfinder_jitter_buffer": {"depth": 1}}"#).is_err());
        assert!(serde_json::from_str::<RawFrameStreamConfig>(r#"{"viewfinder_pacing": "fifo"}"#).is_err());
    }
}
