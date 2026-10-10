package com.unoone.agent.personal

import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.unoone.agent.core.guardian.*

/** Warning / receipt card (brief §3.6). Deterministic host copy only; model notes are labelled as not authority.
 * `onAcknowledge` is offered only for WARN and carries the exact fingerprint shown. `onCorrect` records the
 * visible report/correct-warning control through the runtime's tiny API. */
@Composable fun GuardianCard(
    severity: Severity, intentKind: String, fingerprint: String, signals: List<Signal>, explanation: String, verificationRoute: String?,
    modelNote: String? = null, decidedBy: DecidedBy? = null, proceeded: Boolean? = null,
    onAcknowledge: ((String) -> Unit)? = null, onCorrect: ((CorrectionKind, String) -> Unit)? = null, enabled: Boolean = true,
) {
    var reporting by remember { mutableStateOf(false) }
    var comment by remember { mutableStateOf("") }
    val colour = when (severity) { Severity.BLOCK -> MaterialTheme.colorScheme.error; Severity.WARN -> MaterialTheme.colorScheme.tertiary; Severity.ALLOW -> MaterialTheme.colorScheme.primary }
    Card(Modifier.fillMaxWidth()) { Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text("Privacy guardian · ${severity.name} · ${intentKind.lowercase().replace('_', ' ')}", color = colour, style = MaterialTheme.typography.titleMedium)
        Text(when (severity) { Severity.ALLOW -> "No warning."; Severity.WARN -> "Check before continuing — your decision is needed."; Severity.BLOCK -> "Stopped. This action stays unavailable from the assistant." })
        Text(explanation)
        verificationRoute?.let { Text("Independent check: $it") }
        modelNote?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
        if (decidedBy != null) Text("Decided by ${if (decidedBy == DecidedBy.HUMAN) "you (explicit review)" else "local policy"} · ${if (proceeded == true) "action proceeded" else "action not performed"}", style = MaterialTheme.typography.bodySmall)
        Text("Signals: ${signals.joinToString(", ") { it.name }}\nFingerprint: $fingerprint", style = MaterialTheme.typography.bodySmall)
        if (severity == Severity.WARN && onAcknowledge != null) Button(onClick = { onAcknowledge(fingerprint) }, enabled = enabled) { Text("I checked this independently — proceed once with exactly this destination") }
        if (onCorrect != null) {
            TextButton(onClick = { reporting = !reporting }, enabled = enabled) { Text(if (reporting) "Close report" else "Report / correct this warning") }
            if (reporting) {
                OutlinedTextField(comment, { if (it.length <= 500) comment = it }, label = { Text("What happened (optional; secrets are masked)") }, enabled = enabled)
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    TextButton(onClick = { onCorrect(CorrectionKind.FALSE_ALARM, comment); reporting = false; comment = "" }, enabled = enabled) { Text("Needless warning") }
                    TextButton(onClick = { onCorrect(CorrectionKind.CONFIRMED_HARMFUL, comment); reporting = false; comment = "" }, enabled = enabled) { Text("It was harmful") }
                    TextButton(onClick = { onCorrect(CorrectionKind.MISSED_WARNING, comment); reporting = false; comment = "" }, enabled = enabled) { Text("Something harmful was missed") }
                }
                Text("Corrections are recorded in your encrypted ledger. They never change a grant or silence future checks.", style = MaterialTheme.typography.bodySmall)
            }
        }
    } }
}

@Composable fun GuardianReceiptCard(receipt: Receipt, onCorrect: ((CorrectionKind, String) -> Unit)?, enabled: Boolean = true) =
    GuardianCard(receipt.severity, receipt.intent_kind, receipt.fingerprint, receipt.signals, receipt.explanation, receipt.verification_route, receipt.model_note, receipt.decided_by, receipt.proceeded, null, onCorrect, enabled)
