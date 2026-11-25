# Хаптика экрана камеры

Часть спецификации экрана камеры (сессия 09). Читатель — сессии 12, 13 и 15 (какое событие что играет). Пометки происхождения — см. [README](README.md).

## 1. Что говорит Apple

`[doc]` HIG Playing haptics ([ссылка](https://developer.apple.com/design/human-interface-guidelines/playing-haptics)):
- *"Selection haptics provide feedback while the values of a UI element are changing."* — щелчки при прокрутке дискретных значений (колесо зума, карусель режимов).
- *"Impact haptics provide a physical metaphor you can use to complement a visual experience. For example, people might feel a tap when a view snaps into place or a thud when two heavy objects collide."*
- *"Notification haptics provide feedback about the outcome of a task or action"* — успех, предупреждение, ошибка.
- *"Use system-provided haptic patterns according to their documented meanings."*, *"Avoid overusing haptics."*, *"Make haptics optional."*
- *"Ensure that haptic vibrations don't disrupt experiences involving device features like the camera, gyroscope, or microphone."* — во время будущей записи видео вибрацию ослаблять или выключать (план `video`).

`[doc]` UIKit: `UISelectionFeedbackGenerator` — *"Use selection feedback to communicate movement through a series of discrete values."*; `UIImpactFeedbackGenerator` — *"Use impact feedback to indicate when an impact occurs."*; стиль Light — *"A collision between small, light user interface elements."*, Medium — *"moderately sized"*, Soft — *"exhibiting a large amount of compression or elasticity."*

**Какие события штатной камеры iPhone дают вибрацию, Apple нигде не пишет.** Обзоры подтверждают только аппаратную кнопку Camera Control: `[review]` [MacRumors](https://www.macrumors.com/how-to/iphone-using-camera-control-button/): *"a light press on the button engages a haptic click"*. Вибрация в записи экрана не видна. Поэтому таблица ниже — `[estimate]` по смыслу паттернов HIG, проверка — на эталонном iPhone (список в `results/09.md`).

## 2. Что умеет наш телефон

- Факты 00 и 02: примитивов (`TICK`, `LOW_TICK`, `CLICK`, `THUD`) нет, готовые эффекты `TICK`, `CLICK`, `HEAVY_CLICK`, `DOUBLE_CLICK` поддержаны (`YES`), амплитуда управляется.
- План хаптики (`crates/android-shell/src/haptic_plan.rs`, `plan_haptic`): примитив → готовый эффект с поддержкой `YES` → своя амплитудная волна → … Поэтому на этом телефоне **все 9 паттернов `HapticPattern` играют одним из 4 готовых эффектов**, настроенные волны не используются. Это решение 28 («кайфово, хай буде»); различать все 9 — бэклог README («Хаптика: … свои амплитудные волны»).
- Системная настройка «вибрация при касании» соблюдается (решение 28).

| `HapticPattern` | Чем играет сейчас |
|---|---|
| `SelectionTick`, `LightImpact`, `SoftImpact` | `EFFECT_TICK` |
| `MediumImpact`, `RigidImpact` | `EFFECT_CLICK` |
| `HeavyImpact`, `Error` | `EFFECT_HEAVY_CLICK` |
| `Success`, `Warning` | `EFFECT_DOUBLE_CLICK` |

## 3. Событие → паттерн

| Событие | Паттерн iOS (вероятно) | Наш `HapticPattern` | Ощущение у нас | Происхождение |
|---|---|---|---|---|
| Колесо зума: проход каждого тика | selection | `SelectionTick` | TICK | `[estimate]` по HIG «series of discrete values» |
| Колесо зума: проход «опорного» значения (.5, 1×, 2) | impact light / rigid | `RigidImpact` | CLICK — отличается от тика | `[estimate]` |
| Тап по кнопке зума | — (без вибрации) | нет | — | `[estimate]`: кнопка, HIG «avoid overusing» |
| Карусель режимов: прилипание к новому режиму | selection | `SelectionTick` | TICK | `[estimate]` |
| AE/AF Lock сработал (долгий тап) | impact medium | `MediumImpact` | CLICK | `[estimate]`: «view snaps into place» |
| Долгое нажатие на кнопку белого (WB32) | — | `MediumImpact` | CLICK | наше, в iOS такой кнопки нет; отказ источника — `Error` (HEAVY_CLICK) |
| Ползунок экспозиции: шаги | selection | `SelectionTick` | TICK | `[estimate]` |
| Ползунок экспозиции: проход через 0 | impact light | `RigidImpact` | CLICK — иначе не отличить от шага | `[estimate]` |
| Затвор | нет вибрации (есть звук и моргание) | нет | — | `[estimate]`: HIG «don't disrupt … the camera» — вибрация в момент экспозиции смазывает кадр |
| Меню Controls открылось/закрылось | — | нет | — | `[estimate]` |
| Таймер: последние секунды отсчёта | — | нет | — | `[estimate]`; отсчёт — на экране |
| Ошибка снимка (не сохранён) | notification error | `Error` | HEAVY_CLICK | `[estimate]` |

Правило для 12–15: вибрация — только на дискретные события, которые палец «чувствует» (прилипание, переход опорного значения, срабатывание блокировки). На кнопки и затвор — нет. Исключение — кнопка белого (решение WB32 плана white-balance): долгое нажатие и отказ источника.
