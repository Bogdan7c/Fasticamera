// Модель основы тона снимка как КОНСТАНТА СПЕЦИАЛИЗАЦИИ (сессия 16a, `photo_output_rgba8.comp`).
//
// Общий `tone_base.glsl` отдаёт номер модели из uniform-блока на лету (`tone_base_model()`), поэтому выходной
// проход тащит код всех четырёх моделей и ветвится по ним. Здесь функция подменяется: оригинал из общего файла
// переименован макросом (и остаётся неиспользуемым), а `tone_base_model()` возвращает константу специализации.
// Общие файлы не правятся (их делят видоискатель и качественный путь V4) — меняется только то, что видит ЭТОТ шейдер.
//
// Формулы не затронуты: меняется лишь то, какие ветки `if (tone_base_model() == …)` остаются в коде.
//
// Включать ДО `quality_output_main.glsl`, один раз.
#ifndef PHOTO_TONE_MODEL_GLSL
#define PHOTO_TONE_MODEL_GLSL

// Номер привязки блока основы тона — тот же, что объявит `quality_output_main.glsl` (идентичное переопределение).
#define TONE_BASE_PARAMS_BINDING 5

#define tone_base_model tone_base_model_from_block
#include "tone_base.glsl"
#undef tone_base_model

// Номер модели (`TONE_MODEL_*` в `tone_base.glsl`): значение задаёт Rust при создании пайплайна.
layout(constant_id = 2) const uint PHOTO_TONE_MODEL = 0u;

uint tone_base_model() {
    return PHOTO_TONE_MODEL;
}

#endif
