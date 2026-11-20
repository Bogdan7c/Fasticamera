// Ступень «вниз» dual Kawase: пиксель уровня k+1 = среднее по 5 билинейным выборкам ступени k
// (центр ×4 и четыре диагонали на полтексела выхода), деление на 8. Позиции — в ЛОГИЧЕСКИХ пикселях экрана.
// Подключающий файл обязан объявить функцию `vec3 source_color(vec2 screen_px)`.

layout(push_constant) uniform BlurParams {
    vec4 destination;  // размещение фона (x, y, ширина, высота), логические px — для источника-фона
    vec4 steps;        // x: сдвиг диагональных выборок, px; y: px экрана на тексель цели; z: px на тексель источника-уровня
    vec4 window;       // окно чтения фона: origin.xy, step.xy (первая ступень)
    vec4 content;      // x, y: размер содержимого фона, текселей (первая ступень)
} blur;

layout(location = 0) out vec4 out_color;

void main() {
    vec2 screen = gl_FragCoord.xy * blur.steps.y;
    float offset = blur.steps.x;
    vec3 sum = source_color(screen) * 4.0;
    sum += source_color(screen + vec2(-offset, -offset));
    sum += source_color(screen + vec2(offset, offset));
    sum += source_color(screen + vec2(offset, -offset));
    sum += source_color(screen + vec2(-offset, offset));
    out_color = vec4(sum / 8.0, 1.0);
}
