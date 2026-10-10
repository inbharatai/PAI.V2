package com.unoone.agent

import kotlin.coroutines.AbstractCoroutineContextElement
import kotlin.coroutines.CoroutineContext

/** Transcript identity only; deliberately confers no native task or voice approval authority. */
internal class ConversationSession(val id: String, val inputType: String) : AbstractCoroutineContextElement(Key) {
    companion object Key : CoroutineContext.Key<ConversationSession>
}
