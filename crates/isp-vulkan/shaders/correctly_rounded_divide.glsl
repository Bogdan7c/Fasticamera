// Деление f32 с правильным округлением (общее для качественного конвейера, проявления снимка и
// коррекции виньетирования). Подключается через #include; защита от повторного включения — ниже.
#ifndef CORRECTLY_ROUNDED_DIVIDE_GLSL
#define CORRECTLY_ROUNDED_DIVIDE_GLSL

// Деление с округлением как на CPU (IEEE 754, к ближайшему). OpFDiv в Vulkan гарантирует лишь 2,5 ULP,
// а решение H/V демозаика — сравнение: разница в один ULP на входе переворачивает направление у почти
// равных оценок и даёт расхождение ~1e-3 (замерено на зонной пластинке). `fma()` тут не помогает:
// спецификация Vulkan разрешает считать его как отдельные умножение и сложение (без слияния).
// Зато OpFMul/OpFAdd/OpFSub округляются правильно, поэтому точное произведение q·d строится по Деккеру
// (расщепление Велткампа), остаток a − q·d получается без ошибки, и одна поправка q + остаток/d
// доводит частное до правильно округлённого (кроме случаев ровно посередине между соседними f32 и
// денормалов — в нормированном свете их нет).
const float VELTKAMP_SPLITTER = 4097.0; // 2^12 + 1: делит 24-битную мантиссу на две половины по 12 бит

void split_float(float value, out float high, out float low) {
    precise float scaled = VELTKAMP_SPLITTER * value;
    precise float high_part = scaled - (scaled - value);
    precise float low_part = value - high_part;
    high = high_part;
    low = low_part;
}

float divide_correctly_rounded(float numerator, float denominator) {
    precise float quotient = numerator / denominator;
    float quotient_high;
    float quotient_low;
    float denominator_high;
    float denominator_low;
    split_float(quotient, quotient_high, quotient_low);
    split_float(denominator, denominator_high, denominator_low);
    // Точное произведение quotient·denominator = product + product_error.
    precise float product = quotient * denominator;
    precise float product_error = (((quotient_high * denominator_high - product) + quotient_high * denominator_low)
        + quotient_low * denominator_high) + quotient_low * denominator_low;
    // numerator − product точно (Стербенц: product отличается от numerator меньше чем вдвое).
    precise float residual = (numerator - product) - product_error;
    precise float corrected = quotient + residual / denominator;
    return corrected;
}

#endif
