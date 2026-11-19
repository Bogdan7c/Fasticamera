package dev.fasticamera

import android.content.Context
import android.graphics.Color
import android.graphics.Typeface
import android.view.Gravity
import android.view.View
import android.view.WindowInsets
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.TextView

/**
 * Плашка с подсказкой владельцу («ЩЁЛКНИ ЛАМПОЙ», «ЗАКРОЙ ОБЪЕКТИВ РУКОЙ») вместо вибрации в
 * smoke-сценариях, где человек что-то делает руками.
 *
 * Логики нет: тексты формирует Rust (`shell-contract::operator_prompt`), сюда они приходят готовыми.
 * Единственное решение здесь — цвет фона по виду подсказки (коды `PROMPT_KIND_*`, контракт с
 * `crates/android-shell/src/command_codes.rs`). Плашка стоит в нижней части экрана, НЕ по центру:
 * сценарий `--neutral-card` меряет по скриншоту именно центр кадра. В покое она `GONE` и не занимает место.
 * Все методы вызываются только с главного потока (`ShellPlatform.onMainThread`).
 */
class OperatorPromptView(context: Context) : LinearLayout(context) {
    private val headlineView = TextView(context)
    private val detailView = TextView(context)

    init {
        orientation = VERTICAL
        gravity = Gravity.CENTER_HORIZONTAL
        visibility = View.GONE
        val padding = dp(PLATE_PADDING_DP)
        setPadding(padding, padding, padding, padding)

        headlineView.apply {
            setTextColor(Color.WHITE)
            setTextSize(android.util.TypedValue.COMPLEX_UNIT_SP, HEADLINE_SP)
            setTypeface(typeface, Typeface.BOLD)
            gravity = Gravity.CENTER
        }
        detailView.apply {
            setTextColor(Color.WHITE)
            setTextSize(android.util.TypedValue.COMPLEX_UNIT_SP, DETAIL_SP)
            gravity = Gravity.CENTER
        }
        addView(headlineView)
        addView(detailView)

        // Панели скрыты, но вырез и жестовая полоса остаются: плашка не должна уезжать под них.
        // Отступ от нижнего края безопасной зоны держит плашку в нижней трети, вдали от центра кадра.
        layoutParams = FrameLayout.LayoutParams(
            FrameLayout.LayoutParams.MATCH_PARENT,
            FrameLayout.LayoutParams.WRAP_CONTENT,
            Gravity.BOTTOM,
        )
        setOnApplyWindowInsetsListener { view, insets ->
            val safe = insets.getInsets(WindowInsets.Type.systemBars() or WindowInsets.Type.displayCutout())
            (view.layoutParams as FrameLayout.LayoutParams).apply {
                setMargins(safe.left, 0, safe.right, safe.bottom + dp(BOTTOM_MARGIN_DP))
            }
            view.requestLayout()
            insets
        }
    }

    /** Показывает подсказку; следующий вызов заменяет предыдущую. [kind] — `PROMPT_KIND_*`. */
    fun show(headline: String, detail: String, kind: Int) {
        // Неизвестный вид — ошибка контракта: исключение уйдёт в Rust отказом команды, а не тихим цветом по умолчанию.
        setBackgroundColor(backgroundFor(kind))
        headlineView.text = headline
        detailView.text = detail
        // Пустое пояснение не занимает строку.
        detailView.visibility = if (detail.isEmpty()) View.GONE else View.VISIBLE
        visibility = View.VISIBLE
    }

    /** Убирает подсказку; без показанной — безвредно. */
    fun clear() {
        visibility = View.GONE
        headlineView.text = ""
        detailView.text = ""
    }

    private fun backgroundFor(kind: Int): Int = when (kind) {
        PROMPT_KIND_PREPARE -> COLOR_PREPARE
        PROMPT_KIND_ACT -> COLOR_ACT
        PROMPT_KIND_DONE -> COLOR_DONE
        PROMPT_KIND_INFO -> COLOR_INFO
        else -> throw IllegalArgumentException("неизвестный вид подсказки: $kind")
    }

    private fun dp(value: Int): Int = (value * resources.displayMetrics.density).toInt()

    companion object {
        /** Виды подсказки — контракт с `command_codes::prompt_kind_code`. */
        const val PROMPT_KIND_PREPARE = 1
        const val PROMPT_KIND_ACT = 2
        const val PROMPT_KIND_DONE = 3
        const val PROMPT_KIND_INFO = 4

        /** Цвета плотные (без прозрачности): читаются на любом кадре видоискателя. */
        private val COLOR_PREPARE = Color.rgb(0xB2, 0x6A, 0x00)
        private val COLOR_ACT = Color.rgb(0xC6, 0x28, 0x28)
        private val COLOR_DONE = Color.rgb(0x2E, 0x7D, 0x32)
        private val COLOR_INFO = Color.rgb(0x37, 0x47, 0x4F)

        /** Крупный шрифт: читается с 1-2 м, пока владелец держит лампу или руку у объектива. */
        private const val HEADLINE_SP = 40f
        private const val DETAIL_SP = 20f
        private const val PLATE_PADDING_DP = 16
        private const val BOTTOM_MARGIN_DP = 48
    }
}
