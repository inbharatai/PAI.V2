package com.unoone.agent.localbrain

import com.google.ai.edge.litertlm.OpenApiTool
import com.google.ai.edge.litertlm.ToolProvider
import com.google.ai.edge.litertlm.tool
import com.unoone.agent.core.model.CanonicalToolRegistry
import com.unoone.agent.core.model.ToolParamType
import com.unoone.agent.core.model.ToolSchema
import com.unoone.agent.core.util.Logger

/**
 * Creates [ToolProvider] instances for only the candidate tools, bypassing [UnoOneToolSet]'s
 * reflection-based registration that exposes ALL 42 tools to the model.
 *
 * Uses LiteRT-LM's [OpenApiTool] interface to register only the tools needed for a specific task,
 * reducing context window waste and improving tool-call accuracy. The model sees only 2–6 tool
 * signatures per turn instead of 42, which is critical for the E2B model's 2,048-token context.
 *
 * Tool descriptors are built from [CanonicalToolRegistry] schemas — the single source of truth for
 * tool names, descriptions, and parameter types. This eliminates the duplication between
 * [UnoOneToolSet]'s `@Tool`/`@ToolParam` annotations and the canonical registry.
 */
object DynamicToolProvider {

    /**
     * Creates a list of [ToolProvider] instances for the given tool names, using
     * [CanonicalToolRegistry] schemas to build OpenAPI tool descriptors.
     *
     * Always includes [CanonicalToolRegistry.ALWAYS_INCLUDED] tools (currently just `speak_response`)
     * so the model can always fall back to a spoken response.
     *
     * @param toolNames the set of canonical tool names to register. Must be a subset of
     *   [CanonicalToolRegistry.names]. Unknown names are logged and skipped.
     * @return a list of [ToolProvider] instances, one per valid tool.
     */
    fun createToolProviders(toolNames: Set<String>): List<ToolProvider> {
        val expandedNames = toolNames + CanonicalToolRegistry.ALWAYS_INCLUDED
        return expandedNames.mapNotNull { name ->
            val schema = CanonicalToolRegistry.schemaFor(name)
            if (schema == null) {
                Logger.w("DynamicToolProvider: unknown tool '$name'; skipping")
                return@mapNotNull null
            }
            try {
                tool(CanonicalOpenApiTool(schema))
            } catch (e: Exception) {
                Logger.e("DynamicToolProvider: failed to create tool provider for '$name'", e)
                null
            }
        }
    }

    /**
     * Creates a list of [ToolProvider] instances for ALL canonical tools.
     * Equivalent to `createToolProviders(CanonicalToolRegistry.names)`.
     */
    fun createAllToolProviders(): List<ToolProvider> =
        createToolProviders(CanonicalToolRegistry.names)
}

/**
 * An [OpenApiTool] implementation that builds its JSON descriptor from a [ToolSchema] in
 * [CanonicalToolRegistry]. This bypasses LiteRT-LM's `@Tool` annotation scanning entirely —
 * only the tools explicitly passed to [DynamicToolProvider.createToolProviders] are registered
 * with the model, so the context window is not polluted with irrelevant tool signatures.
 *
 * The [execute] method is a no-op because UnoOne uses manual tool calling
 * (`automaticToolCalling = false`): the model proposes tool calls as JSON, and the app parses
 * and executes them through [com.unoone.agent.execution.ActionExecutor] after safety validation.
 */
private class CanonicalOpenApiTool(
    private val schema: ToolSchema
) : OpenApiTool {

    /**
     * Returns the tool descriptor as a JSON string in the OpenAI function-calling format that
     * LiteRT-LM's Gemma models expect. Format:
     * ```json
     * {
     *   "name": "create_note",
     *   "description": "Create a local note",
     *   "parameters": {
     *     "type": "object",
     *     "properties": {
     *       "title": { "type": "string", "description": "Short title for the note" },
     *       "content": { "type": "string", "description": "Full note content" }
     *     },
     *     "required": ["title", "content"]
     *   }
     * }
     * ```
     */
    override fun getToolDescriptionJsonString(): String {
        val sb = StringBuilder()
        sb.append("{\"name\":\"").append(escapeJson(schema.name)).append("\"")
        sb.append(",\"description\":\"").append(escapeJson(schema.description)).append("\"")

        if (schema.params.isEmpty()) {
            // No parameters — empty object schema
            sb.append(",\"parameters\":{\"type\":\"object\",\"properties\":{},\"required\":[]}}")
        } else {
            sb.append(",\"parameters\":{\"type\":\"object\",\"properties\":{")
            schema.params.forEachIndexed { i, param ->
                if (i > 0) sb.append(",")
                sb.append("\"").append(escapeJson(param.name)).append("\":{")
                sb.append("\"type\":\"").append(paramTypeToJsonSchema(param.type)).append("\"")
                val paramDesc = param.description
                if (paramDesc != null) {
                    sb.append(",\"description\":\"").append(escapeJson(paramDesc)).append("\"")
                }
                sb.append("}")
            }
            sb.append("},\"required\":[")
            val required = schema.params.filter { it.required }
            required.forEachIndexed { i, param ->
                if (i > 0) sb.append(",")
                sb.append("\"").append(escapeJson(param.name)).append("\"")
            }
            sb.append("]}}")
        }
        return sb.toString()
    }

    /**
     * No-op: UnoOne uses manual tool calling (`automaticToolCalling = false`), so this method
     * is never invoked by LiteRT-LM. Tool execution is handled by
     * [com.unoone.agent.execution.ActionExecutor] after safety validation.
     */
    override fun execute(paramsJsonString: String): String = ""

    /** Maps [ToolParamType] to JSON Schema type strings. */
    private fun paramTypeToJsonSchema(type: ToolParamType): String = when (type) {
        ToolParamType.STRING -> "string"
        ToolParamType.INT -> "integer"
        ToolParamType.BOOLEAN -> "boolean"
        ToolParamType.FLOAT -> "number"
        ToolParamType.DOUBLE -> "number"
        ToolParamType.STRING_LIST -> "array"
    }

    /** Escapes special characters in a JSON string value. */
    private fun escapeJson(s: String): String = s
        .replace("\\", "\\\\")
        .replace("\"", "\\\"")
        .replace("\n", "\\n")
        .replace("\r", "\\r")
        .replace("\t", "\\t")
}