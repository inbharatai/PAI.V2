package com.unoone.agent.core.agent

import com.unoone.agent.core.model.CanonicalToolRegistry
import com.unoone.agent.core.model.ToolCall
import com.unoone.agent.core.model.ToolParamType
import com.unoone.agent.core.model.ToolSchema

/**
 * Validates tool proposals from the model against the candidate tool set and canonical schemas.
 *
 * Defense in depth: the model may propose tool names or arguments that are not in the
 * candidate set for this task. This validator rejects any such proposals before they reach
 * the safety pipeline or executor.
 *
 * The validation chain is:
 * 1. Is the tool name known to [CanonicalToolRegistry]?
 * 2. Is the tool name in the candidate set for this task?
 * 3. Are all required arguments present?
 * 4. Are argument types correct?
 * 5. (Optional) Are argument values within acceptable ranges?
 */
object ToolProposalValidator {

    /**
     * Result of validating a model-proposed tool call.
     */
    sealed class ValidationResult {
        /** The tool call is valid and within the candidate set. */
        data class Valid(val call: ToolCall) : ValidationResult()

        /** The tool call was rejected. [reason] explains why. */
        data class Rejected(val call: ToolCall, val reason: String) : ValidationResult()
    }

    /**
     * Validate a model-proposed tool call against the candidate tool set and canonical schemas.
     *
     * @param call the model's proposed tool call.
     * @param candidateTools the tools registered for this task (from [CandidateToolSelector]).
     * @return [ValidationResult.Valid] if the call passes all checks, [ValidationResult.Rejected] otherwise.
     */
    fun validate(call: ToolCall, candidateTools: List<ToolSchema>): ValidationResult {
        val toolName = call.tool

        // Check 1: Is the tool known to the canonical registry?
        if (!CanonicalToolRegistry.isKnown(toolName)) {
            return ValidationResult.Rejected(call, "Unknown tool: $toolName")
        }

        // Check 2: Is the tool in the candidate set for this task?
        val candidateNames = candidateTools.map { it.name }.toSet()
        if (toolName !in candidateNames) {
            // speak_response is always allowed as a fallback
            if (toolName != "speak_response") {
                return ValidationResult.Rejected(
                    call,
                    "Tool '$toolName' not in candidate set for this task (candidates: ${candidateNames.sorted().joinToString(", ")})"
                )
            }
        }

        // Check 3: Are all required arguments present?
        val schema = CanonicalToolRegistry.schemaFor(toolName)
            ?: return ValidationResult.Rejected(call, "No schema found for tool: $toolName")

        for (param in schema.params) {
            if (param.required) {
                val value = call.args[param.name]
                if (value == null) {
                    return ValidationResult.Rejected(
                        call,
                        "Missing required argument '${param.name}' for tool '$toolName'"
                    )
                }
            }
        }

        // Check 4: Are argument types correct?
        // Verify that each provided argument's JsonElement type matches the declared param type.
        for (param in schema.params) {
            val argValue = call.args[param.name] ?: continue // null args handled by Check 3
            val typeMatch = when (param.type) {
                ToolParamType.STRING -> argValue is kotlinx.serialization.json.JsonPrimitive && argValue.isString
                    // STRING params must be quoted strings — reject bare integers like "direction": 42
                    // which crash ActionExecutor on .lowercase() / .substring() calls.
                ToolParamType.INT -> argValue is kotlinx.serialization.json.JsonPrimitive &&
                    argValue.content?.toIntOrNull() != null && !argValue.isString
                ToolParamType.BOOLEAN -> argValue is kotlinx.serialization.json.JsonPrimitive &&
                    (argValue.content == "true" || argValue.content == "false")
                ToolParamType.FLOAT -> argValue is kotlinx.serialization.json.JsonPrimitive &&
                    argValue.content?.toFloatOrNull() != null && !argValue.isString
                ToolParamType.DOUBLE -> argValue is kotlinx.serialization.json.JsonPrimitive &&
                    argValue.content?.toDoubleOrNull() != null && !argValue.isString
                ToolParamType.STRING_LIST -> argValue is kotlinx.serialization.json.JsonArray
            }
            // If type doesn't match, reject — including STRING params that arrived as bare numbers.
            // Previously STRING was lenient, but bare integers crash ActionExecutor on .lowercase().
            if (!typeMatch) {
                return ValidationResult.Rejected(
                    call,
                    "Type mismatch for argument '${param.name}' in tool '$toolName': expected ${param.type}, got ${argValue::class.simpleName}"
                )
            }
        }

        return ValidationResult.Valid(call)
    }

    /**
     * Validate a batch of tool proposals, returning valid ones and logging rejections.
     *
     * @param calls the model's proposed tool calls (typically just one, but the model can emit multiple).
     * @param candidateTools the tools registered for this task.
     * @return a pair of (valid calls, rejected calls with reasons).
     */
    fun validateBatch(
        calls: List<ToolCall>,
        candidateTools: List<ToolSchema>
    ): Pair<List<ToolCall>, List<Pair<ToolCall, String>>> {
        val valid = mutableListOf<ToolCall>()
        val rejected = mutableListOf<Pair<ToolCall, String>>()

        for (call in calls) {
            when (val result = validate(call, candidateTools)) {
                is ValidationResult.Valid -> valid.add(result.call)
                is ValidationResult.Rejected -> rejected.add(result.call to result.reason)
            }
        }

        return valid to rejected
    }

    /**
     * Check if a tool name is in the candidate set, allowing speak_response as a universal fallback.
     */
    fun isCandidateOrFallback(toolName: String, candidateTools: List<ToolSchema>): Boolean {
        if (toolName == "speak_response") return true
        return candidateTools.any { it.name == toolName }
    }
}