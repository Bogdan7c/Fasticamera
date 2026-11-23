package dev.fasticamera

import android.app.Activity
import android.app.Application
import android.os.Bundle
import android.app.Dialog
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.text.InputFilter
import android.text.InputType
import android.text.TextWatcher
import android.util.TypedValue
import android.view.Gravity
import android.view.ViewGroup
import android.view.WindowManager
import android.view.inputmethod.EditorInfo
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.TextView

/**
 * Маленький диалог ввода строки с системной клавиатурой (решение 197): заголовок, одно поле, «Cancel» / «Save» в
 * стиле iOS (тёмная скруглённая карточка). Нужен Rust для имени пресета вида; что делать с текстом, диалог не знает.
 *
 * Протокол (контракт с `crates/shell-contract/src/text_input.rs`, коды — `crates/android-shell/src/text_input_wire.rs`):
 * на каждый [show] уходит ровно один ответ в [ShellBridge.nativeTextInputResult] с тем же `id`: введено, отмена
 * («Cancel», «назад», уход Activity на паузу) или «недоступно» (диалог показать не удалось). Новый [show] при
 * открытом диалоге закрывает старый БЕЗ ответа: вытеснение учитывает Rust (`TextInputBroker`).
 * Все методы вызываются только с главного потока (`ShellPlatform.onMainThread`); ответ в Rust не блокирует.
 */
class TextInputDialog(private val activity: Activity) {
    /** Открытый сейчас диалог; `null`, если его нет. Хранит номер, чтобы ответить ровно один раз. */
    private var open: OpenDialog? = null

    /**
     * Activity сейчас на паузе (между `onPause` и следующим `onResume`). `show` приходит через очередь главного
     * потока и может сработать ПОСЛЕ `onPause`: тогда диалог открылся бы поверх фоновой Activity, а
     * [cancelOpenDialog] (он уже отработал на паузе) его бы не закрыл. Флаг ведут колбэки жизненного цикла ЭТОЙ
     * Activity (`Activity.registerActivityLifecycleCallbacks`, API 29+; minSdk проекта 36). По умолчанию `false`:
     * диалог создаётся до первого `onResume`, и «не на паузе» — верное начальное состояние.
     */
    private var paused = false

    init {
        activity.registerActivityLifecycleCallbacks(object : Application.ActivityLifecycleCallbacks {
            override fun onActivityPaused(paused: Activity) {
                this@TextInputDialog.paused = true
            }

            override fun onActivityResumed(resumed: Activity) {
                this@TextInputDialog.paused = false
            }

            override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) = Unit
            override fun onActivityStarted(activity: Activity) = Unit
            override fun onActivityStopped(activity: Activity) = Unit
            override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) = Unit
            override fun onActivityDestroyed(activity: Activity) = Unit
        })
    }

    private class OpenDialog(val id: Long, val dialog: Dialog) {
        /** Ответ уже отправлен (или диалог вытеснен): повторное закрытие ничего не шлёт. */
        var settled = false
    }

    /** Показывает диалог. Любое исключение показа превращается в ответ «недоступно», а не в падение. */
    fun show(id: Long, purpose: Int, initialText: String, maxChars: Int) {
        // Activity уже ушла с переднего плана, пока запрос ждал в очереди: показывать нельзя, а Rust ждёт ответа.
        if (!canShowNow()) {
            dismissSilently()
            ShellBridge.nativeTextInputResult(id, OUTCOME_CANCELLED, null)
            return
        }
        // Вытеснение: старый диалог уходит без ответа (Rust уже закрыл его запрос как вытесненный).
        dismissSilently()
        try {
            val dialog = buildDialog(id, titleFor(purpose), initialText, maxChars)
            open = OpenDialog(id, dialog)
            dialog.show()
        } catch (error: RuntimeException) {
            // Включая неизвестный `purpose` (ошибка контракта): громко для Rust, но без исключения наверх.
            open = null
            ShellBridge.nativeTextInputResult(id, OUTCOME_UNAVAILABLE, null)
        }
    }

    /** Можно ли открывать диалог: Activity на переднем плане и не закрывается. */
    private fun canShowNow(): Boolean = !paused && !activity.isFinishing && !activity.isDestroyed

    /** Activity уходит с переднего плана или уничтожается: открытый диалог закрывается как «отмена». */
    fun cancelOpenDialog() {
        val current = open ?: return
        settle(current, OUTCOME_CANCELLED, null)
        current.dialog.dismiss()
    }

    /** Закрывает открытый диалог без ответа (его запрос уже вытеснен новым). */
    private fun dismissSilently() {
        val current = open ?: return
        current.settled = true
        open = null
        current.dialog.dismiss()
    }

    /** Отправляет ответ в Rust один раз на диалог. */
    private fun settle(target: OpenDialog, outcome: Int, text: String?) {
        if (target.settled) return
        target.settled = true
        if (open === target) open = null
        ShellBridge.nativeTextInputResult(target.id, outcome, text)
    }

    private fun buildDialog(id: Long, title: String, initialText: String, maxChars: Int): Dialog {
        val dialog = Dialog(activity)
        val field = buildField(initialText, maxChars)
        val saveButton = buildButton("Save", bold = true)
        val cancelButton = buildButton("Cancel", bold = false)

        val buttons = LinearLayout(activity).apply {
            orientation = LinearLayout.HORIZONTAL
            addView(cancelButton, LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f))
            addView(saveButton, LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f))
        }
        val card = LinearLayout(activity).apply {
            orientation = LinearLayout.VERTICAL
            background = GradientDrawable().apply {
                setColor(COLOR_CARD)
                cornerRadius = dp(CARD_RADIUS_DP).toFloat()
            }
            setPadding(dp(CARD_PADDING_DP), dp(CARD_PADDING_DP), dp(CARD_PADDING_DP), dp(CARD_PADDING_DP / 2))
            addView(
                TextView(activity).apply {
                    text = title
                    setTextColor(Color.WHITE)
                    setTextSize(TypedValue.COMPLEX_UNIT_SP, TITLE_SP)
                    setTypeface(typeface, Typeface.BOLD)
                    gravity = Gravity.CENTER
                },
                LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT),
            )
            addView(field, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT).apply {
                topMargin = dp(CARD_PADDING_DP)
                bottomMargin = dp(CARD_PADDING_DP / 2)
            })
            addView(buttons)
        }
        dialog.setContentView(card)
        dialog.window?.apply {
            // Окно диалога прозрачное: видна только наша скруглённая карточка.
            setBackgroundDrawable(GradientDrawable().apply { setColor(Color.TRANSPARENT) })
            setLayout(dp(DIALOG_WIDTH_DP), ViewGroup.LayoutParams.WRAP_CONTENT)
            // Клавиатура поднимается сама, а окно сдвигается над ней (документация: SOFT_INPUT_STATE_VISIBLE
            // срабатывает при фокусе на поле, которое принимает текст).
            setSoftInputMode(
                WindowManager.LayoutParams.SOFT_INPUT_STATE_VISIBLE or
                    WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE,
            )
        }
        // «Назад» закрывает диалог (это отмена), касание вне карточки — нет: случайный промах не должен терять ввод.
        dialog.setCanceledOnTouchOutside(false)

        // Сохранить можно только непустое после обрезки краёв (та же проверка в Rust — защитный рубеж).
        fun refreshSaveEnabled() {
            val enabled = field.text.toString().trim().isNotEmpty()
            saveButton.isEnabled = enabled
            saveButton.alpha = if (enabled) 1f else DISABLED_ALPHA
        }
        refreshSaveEnabled()
        field.addTextChangedListener(object : TextWatcher {
            override fun beforeTextChanged(text: CharSequence?, start: Int, count: Int, after: Int) = Unit
            override fun onTextChanged(text: CharSequence?, start: Int, before: Int, count: Int) = Unit
            override fun afterTextChanged(editable: android.text.Editable?) = refreshSaveEnabled()
        })

        val submit = {
            val text = field.text.toString().trim()
            val current = open
            if (text.isNotEmpty() && current != null && current.dialog === dialog) {
                settle(current, OUTCOME_ENTERED, text)
                dialog.dismiss()
            }
        }
        saveButton.setOnClickListener { submit() }
        cancelButton.setOnClickListener {
            open?.takeIf { it.dialog === dialog }?.let { settle(it, OUTCOME_CANCELLED, null) }
            dialog.dismiss()
        }
        // Кнопка «Done» на клавиатуре = «Save» (если поле не пустое).
        field.setOnEditorActionListener { _, actionId, _ ->
            if (actionId == EditorInfo.IME_ACTION_DONE) {
                submit()
                true
            } else {
                false
            }
        }
        // Любое другое закрытие («назад», система) — отмена; после ответа или вытеснения `settled` уже true.
        dialog.setOnDismissListener {
            open?.takeIf { it.dialog === dialog }?.let { settle(it, OUTCOME_CANCELLED, null) }
        }
        return dialog
    }

    private fun buildField(initialText: String, maxChars: Int): EditText = EditText(activity).apply {
        setTextColor(Color.WHITE)
        setHintTextColor(COLOR_HINT)
        setTextSize(TypedValue.COMPLEX_UNIT_SP, FIELD_SP)
        inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_CAP_SENTENCES
        setSingleLine(true)
        imeOptions = EditorInfo.IME_ACTION_DONE
        // Предел длины: пользователь просто не может напечатать лишнее (Rust всё равно обрезает ответ).
        filters = arrayOf(InputFilter.LengthFilter(maxChars.coerceAtLeast(1)))
        background = GradientDrawable().apply {
            setColor(COLOR_FIELD)
            cornerRadius = dp(FIELD_RADIUS_DP).toFloat()
        }
        setPadding(dp(FIELD_PADDING_DP), dp(FIELD_PADDING_DP), dp(FIELD_PADDING_DP), dp(FIELD_PADDING_DP))
        setText(initialText.take(maxChars.coerceAtLeast(1)))
        // Курсор в конце, всё выделено не нужно: iOS при переименовании ставит курсор в конец.
        setSelection(text.length)
        requestFocus()
    }

    private fun buildButton(label: String, bold: Boolean): TextView = TextView(activity).apply {
        text = label
        setTextColor(COLOR_ACCENT)
        setTextSize(TypedValue.COMPLEX_UNIT_SP, BUTTON_SP)
        if (bold) setTypeface(typeface, Typeface.BOLD)
        gravity = Gravity.CENTER
        setPadding(0, dp(BUTTON_PADDING_DP), 0, dp(BUTTON_PADDING_DP))
        isClickable = true
    }

    private fun titleFor(purpose: Int): String = when (purpose) {
        PURPOSE_NEW_PRESET_NAME -> "New Preset"
        PURPOSE_RENAME_PRESET -> "Rename Preset"
        else -> throw IllegalArgumentException("неизвестное назначение диалога: $purpose")
    }

    private fun dp(value: Int): Int = (value * activity.resources.displayMetrics.density).toInt()

    companion object {
        /** Назначения — контракт с `text_input_wire::text_purpose_code`. */
        const val PURPOSE_NEW_PRESET_NAME = 1
        const val PURPOSE_RENAME_PRESET = 2

        /** Итоги — контракт с `text_input_wire::OUTCOME_*`. */
        const val OUTCOME_ENTERED = 1
        const val OUTCOME_CANCELLED = 2
        const val OUTCOME_UNAVAILABLE = 3

        private val COLOR_CARD = Color.rgb(0x2C, 0x2C, 0x2E)
        private val COLOR_FIELD = Color.rgb(0x1C, 0x1C, 0x1E)
        private val COLOR_HINT = Color.rgb(0x8E, 0x8E, 0x93)
        private val COLOR_ACCENT = Color.rgb(0x0A, 0x84, 0xFF)
        private const val DISABLED_ALPHA = 0.4f

        private const val DIALOG_WIDTH_DP = 300
        private const val CARD_RADIUS_DP = 14
        private const val CARD_PADDING_DP = 16
        private const val FIELD_RADIUS_DP = 8
        private const val FIELD_PADDING_DP = 10
        private const val BUTTON_PADDING_DP = 12
        private const val TITLE_SP = 17f
        private const val FIELD_SP = 17f
        private const val BUTTON_SP = 17f
    }
}
