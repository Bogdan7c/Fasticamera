//! Диагностика цветовых приборов поверх видоискателя (сессия 14h).
//!
//! Разделение по времени жизни:
//! * [`ScopeCounters`] — накопительные счётчики; их копит статистика видоискателя и они переживают пересоздание
//!   цепочки показа (объекты GPU приборов принадлежат цепочке и уходят вместе с ней);
//! * [`ScopeResidency`] — живое состояние объектов GPU «здесь и сейчас» (есть ли, сколько ждут освобождения);
//! * [`ScopeDiagnostics`] — всё вместе плюс то, что хранят другие владельцы: запрос (настройки) и число
//!   отзывов запроса (бэкенд).

use gpu_vulkan::context::Float32Controls;
use scope_contract::ScopeRequest;

use crate::thread::DurationSummary;

/// Есть ли у приборов объекты GPU (инвариант 2: закрытые приборы ничего не стоят).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ScopeResources {
    /// Объектов нет: приборы закрыты (или ещё ни разу не открывались).
    #[default]
    Absent,
    /// Есть действующий набор объектов, прибор считается.
    Live,
    /// Приборы запрошены, но объекты GPU создать не удалось (создание отказало, правило запроса не решается): повтор
    /// будет только после смены запроса или формата цели. Причина — `ScopeCounters::last_failure`.
    Failed,
    /// Действующего набора нет, но выведенный из обращения ещё ждёт, пока GPU закончит кадры, которые его читают.
    Retiring,
}

/// Накопительные счётчики приборов.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScopeCounters {
    /// Сколько кадров приборов записано в командные буферы (на каждый новый показанный кадр при
    /// `every_nth_frame = 1`).
    pub passes_recorded: u64,
    /// Сколько новых кадров пропущено прореживанием `every_nth_frame` (рычаг бюджета).
    pub frames_skipped_by_cadence: u64,
    /// Сколько новых кадров пропущено, потому что на экран они не попали (удержание, скрытие, чёрный):
    /// прибор меряет только показанную картинку (решение 189).
    pub frames_skipped_not_shown: u64,
    /// Сколько новых кадров пропущено, потому что набор объектов (или картинка нужного размера) ещё не подготовлен
    /// между кадрами: внутри кадра приборы ничего не создают (как рендер UI).
    pub frames_skipped_not_ready: u64,
    /// Сколько новых кадров пропущено, потому что нет входа приборов (нет изображения видоискателя или подложки).
    pub frames_skipped_no_input: u64,
    /// Сколько новых кадров пропущено, потому что приборы заблокированы отказом (см. [`ScopeResources::Failed`]):
    /// отказ учитывается ОДИН раз на смену запроса, а не на каждом кадре.
    pub frames_skipped_blocked: u64,
    /// Сколько раз создан набор объектов GPU приборов.
    pub resources_created: u64,
    /// Сколько наборов объектов GPU уничтожено (после того, как GPU закончил читать их кадры).
    pub resources_released: u64,
    /// Сколько раз прибор не записан или не создан (кадр видоискателя при этом показан). Устойчивый отказ
    /// (создание) считается один раз на смену запроса.
    pub record_failures: u64,
    /// Сколько раз правило прибора не решилось (запрос против настроек и охвата экрана); один раз на смену запроса.
    pub resolve_failures: u64,
    /// Сколько раз картинка прибора отдана рисованию UI.
    pub picture_handoffs: u64,
    /// Причина последнего отказа (создание, решение правила или запись).
    pub last_failure: Option<String>,
}

impl ScopeCounters {
    /// Прибавляет приращение `delta` (из `ScopeEvents`): числа складываются, причина берётся последняя.
    pub fn absorb(&mut self, delta: &ScopeCounters) {
        self.passes_recorded += delta.passes_recorded;
        self.frames_skipped_by_cadence += delta.frames_skipped_by_cadence;
        self.frames_skipped_not_shown += delta.frames_skipped_not_shown;
        self.frames_skipped_not_ready += delta.frames_skipped_not_ready;
        self.frames_skipped_no_input += delta.frames_skipped_no_input;
        self.frames_skipped_blocked += delta.frames_skipped_blocked;
        self.resources_created += delta.resources_created;
        self.resources_released += delta.resources_released;
        self.record_failures += delta.record_failures;
        self.resolve_failures += delta.resolve_failures;
        self.picture_handoffs += delta.picture_handoffs;
        if delta.last_failure.is_some() {
            self.last_failure.clone_from(&delta.last_failure);
        }
    }
}

/// Живое состояние объектов GPU приборов.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScopeResidency {
    /// Есть ли объекты.
    pub resources: ScopeResources,
    /// Сколько пайплайнов накопления у действующего набора (все строятся при создании и больше не меняются: смена
    /// типа и осей на ходу новых не создаёт). 0 — набора нет.
    pub accumulator_pipelines: usize,
    /// Сколько выведенных наборов ждут завершения GPU.
    pub retiring_sets: usize,
}

/// Диагностика приборов в статусе видоискателя.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScopeDiagnostics {
    /// Что сейчас просит экран (`None` — приборы закрыты).
    pub requested: Option<ScopeRequest>,
    /// Объекты GPU сейчас.
    pub residency: ScopeResidency,
    /// Накопительные счётчики.
    pub counters: ScopeCounters,
    /// Факт об устройстве: управление округлением и денормалями f32 в шейдерах. Побитовый паритет счётчиков GPU с
    /// эталоном опирается на предположение «по умолчанию f32 округляется к ближайшему чётному, денормалей на входе
    /// нет» (`isp-vulkan/src/scopes`); здесь видно, что устройство вообще умеет, чтобы расхождение паритета на
    /// новом GPU можно было объяснить, а не угадывать.
    pub float32_controls: Float32Controls,
    /// Время GPU прохода приборов (скользящее окно); `None` — замеров нет.
    pub gpu_time: Option<DurationSummary>,
    /// Сколько раз запрос отозван самим рендером, потому что источник UI ушёл (снят, заменён, сломался): без
    /// источника показывать картинку прибора некому.
    pub withdrawals: u64,
}

/// Приращения, которые цепочка показа отдаёт статистике после записи кадра или сбора слота.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScopeEvents {
    /// Приращения счётчиков.
    pub counters: ScopeCounters,
    /// Время GPU проходов, собранных со слота (нс).
    pub gpu_times_ns: Vec<u64>,
}

impl ScopeEvents {
    /// Ничего не произошло.
    pub fn is_empty(&self) -> bool {
        self.counters == ScopeCounters::default() && self.gpu_times_ns.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absorb_adds_numbers_and_keeps_the_latest_failure() {
        let mut total = ScopeCounters {
            passes_recorded: 2,
            last_failure: Some("старая".to_owned()),
            ..ScopeCounters::default()
        };
        total.absorb(&ScopeCounters {
            passes_recorded: 3,
            frames_skipped_not_ready: 2,
            frames_skipped_no_input: 3,
            frames_skipped_blocked: 4,
            record_failures: 1,
            resources_released: 1,
            last_failure: Some("новая".to_owned()),
            ..ScopeCounters::default()
        });
        assert_eq!(
            (total.passes_recorded, total.record_failures, total.resources_released),
            (5, 1, 1)
        );
        assert_eq!(
            (
                total.frames_skipped_not_ready,
                total.frames_skipped_no_input,
                total.frames_skipped_blocked
            ),
            (2, 3, 4)
        );
        assert_eq!(total.last_failure.as_deref(), Some("новая"));
        total.absorb(&ScopeCounters::default());
        assert_eq!(
            total.last_failure.as_deref(),
            Some("новая"),
            "пустое приращение причину не стирает"
        );
    }

    #[test]
    fn events_report_emptiness_by_both_parts() {
        assert!(ScopeEvents::default().is_empty());
        let with_time = ScopeEvents {
            gpu_times_ns: vec![5],
            ..ScopeEvents::default()
        };
        assert!(!with_time.is_empty());
    }
}
