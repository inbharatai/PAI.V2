package com.unoone.agent.storage.cache

/** Trusted source 2eb9b9e schema, not SQL read from sqlite_master. Settings are SharedPreferences,
 * not a sixth SQL table; migration deliberately does not touch those files. */
object LegacyRoomSchema {
    data class Column(val name: String, val type: String, val nullable: Boolean = false, val primary: Boolean = false)
    data class Index(val table: String, val column: String, val unique: Boolean = false) {
        val name get() = "index_${table}_$column"
        val sql get() = "CREATE ${if (unique) "UNIQUE " else ""}INDEX `$name` ON `$table` (`$column`)"
    }
    private fun id() = Column("id", "INTEGER", primary = true)
    private fun text(name: String, nullable: Boolean = false) = Column(name, "TEXT", nullable)
    private fun integer(name: String, nullable: Boolean = false) = Column(name, "INTEGER", nullable)
    val tables = linkedMapOf(
        "notes" to listOf(id(), text("title"), text("content"), text("tags"), integer("createdAt"), integer("updatedAt"), integer("reminderTime", true)),
        "memories" to listOf(id(), text("key"), text("value"), text("type"), integer("createdAt"), integer("updatedAt")),
        "skills" to listOf(id(), text("name"), text("triggerPhrases"), text("stepsJson"), integer("riskLevel"), integer("enabled"), integer("createdAt"), integer("updatedAt")),
        "action_logs" to listOf(id(), text("inputText"), text("inputType"), text("selectedTool"), text("toolArgsJson"), integer("riskLevel"), text("status"), text("errorMessage", true), integer("sttLatencyMs", true), integer("modelLatencyMs", true), integer("ttsLatencyMs", true), integer("createdAt")),
        "model_metadata" to listOf(id(), text("modelName"), text("modelType"), text("localPath"), text("checksum"), text("status"), integer("lastLoadedAt", true)),
    )
    val indexes = listOf(Index("notes", "title"), Index("notes", "tags"), Index("notes", "createdAt"),
        Index("action_logs", "status"), Index("action_logs", "createdAt"), Index("memories", "key", true),
        Index("memories", "type"), Index("skills", "name", true))
    fun create(table: String): String = "CREATE TABLE `$table` (" + tables.getValue(table).joinToString(", ") {
        "`${it.name}` ${it.type}" + (if (it.primary) " PRIMARY KEY AUTOINCREMENT" else "") + (if (!it.nullable) " NOT NULL" else "")
    } + ")"
    const val MAX_ROWS = 200_000L
    const val MAX_CELL_BYTES = 1024 * 1024
}
