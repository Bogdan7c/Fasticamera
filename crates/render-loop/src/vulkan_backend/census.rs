//! Перепись живых объектов Vulkan по видам для диагностики бэкенда.

use std::collections::BTreeMap;

use gpu_vulkan::live_objects::{LiveObjectKind, LiveObjectSnapshot};

/// Превращает снимок счётчиков в карту «имя вида -> сколько живых».
///
/// Виды с нулём не попадают в карту: так две карты равны ровно тогда, когда равны снимки, а отчёт
/// не засоряется пятнадцатью строками с нулями. Имя вида — `Debug`-имя варианта (`Fence`, `Image`).
pub(super) fn census_by_kind(snapshot: &LiveObjectSnapshot) -> BTreeMap<String, u64> {
    LiveObjectKind::ALL
        .iter()
        .filter_map(|kind| {
            let count = u64::try_from(snapshot.count(*kind)).unwrap_or(u64::MAX);
            (count > 0).then(|| (format!("{kind:?}"), count))
        })
        .collect()
}
