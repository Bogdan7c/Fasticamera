package dev.fasticamera.probe

import dev.fasticamera.probe.report.LogChunker
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class LogChunkerTest {

    private val chunker = LogChunker(maxCharsPerLine = 1000)

    @Test
    fun `короткая строка проходит без конверта`() {
        val line = """{"kind":"result"}"""
        assertEquals(listOf(line), chunker.split(line, "run-1"))
    }

    @Test
    fun `длинная строка режется и склеивается обратно без потерь`() {
        // Кавычки и обратные слэши раздувают куски при экранировании — проверяем запас.
        val original = JSONObject().put("blob", "\"\\x".repeat(2000)).toString()
        val pieces = chunker.split(original, "run-1")
        assertTrue(pieces.size > 1)
        pieces.forEach { assertTrue("кусок длиной ${it.length} не влез", it.length <= 1000) }
        val envelopes = pieces.map(::JSONObject)
        envelopes.forEachIndexed { index, envelope ->
            assertEquals("run-1", envelope.getString("chunk_of"))
            assertEquals(index, envelope.getInt("part"))
            assertEquals(pieces.size, envelope.getInt("parts"))
        }
        val reassembled = envelopes.joinToString("") { it.getString("text") }
        assertEquals(original, reassembled)
    }

    @Test(expected = IllegalArgumentException::class)
    fun `слишком маленький предел отвергается сразу`() {
        LogChunker(maxCharsPerLine = 100)
    }
}
