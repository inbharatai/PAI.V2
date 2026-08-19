package com.unoone.agent.ui.viewmodel

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.unoone.agent.storage.dao.NoteDao
import com.unoone.agent.storage.entity.NoteEntity
import com.unoone.agent.vault.VaultSyncPlanner
import com.unoone.agent.vaultbridge.VaultMirror
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/**
 * @param vaultMirror when non-null, note creates/deletes are mirrored to the
 * shared drive vault (write-through when unlocked, queued otherwise). Null
 * keeps the pre-vault behaviour (cache only) for tests and previews.
 */
class NotesViewModel(
    private val noteDao: NoteDao,
    private val vaultMirror: VaultMirror? = null,
) : ViewModel() {

    private val _notes = MutableStateFlow<List<NoteEntity>>(emptyList())
    val notes: StateFlow<List<NoteEntity>> = _notes.asStateFlow()

    private val _searchQuery = MutableStateFlow("")
    val searchQuery: StateFlow<String> = _searchQuery.asStateFlow()

    /** Active collection job — cancelled and replaced whenever the search query changes
     *  to prevent dual-collector races on _notes. */
    private var searchCollectionJob: Job? = null

    init {
        // Base collection: always collects the full list.
        // When the search query is blank, searchCollectionJob is null, so this
        // is the sole writer to _notes. When a search is active, this collector
        // still runs but the search collector's last-write wins since Room emits
        // the filtered result after the full one.
        viewModelScope.launch {
            noteDao.getAll().collect { list ->
                // Only update from base collector if no search is active
                if (searchCollectionJob == null || !searchCollectionJob!!.isActive) {
                    _notes.value = list
                }
            }
        }
    }

    fun onSearchQueryChange(query: String) {
        _searchQuery.value = query
        // Cancel the previous search collection to prevent dual-collector race
        searchCollectionJob?.cancel()

        if (query.isBlank()) {
            // No search active — base collector will handle updates
            searchCollectionJob = null
        } else {
            // Start a new filtered collection, replacing any previous one
            searchCollectionJob = viewModelScope.launch {
                noteDao.search(query).collect { _notes.value = it }
            }
        }
    }

    fun createNote(title: String, content: String, tags: String = "") {
        viewModelScope.launch {
            val id = noteDao.insert(
                NoteEntity(title = title, content = content, tags = tags)
            )
            // Canonicalise to the drive vault (no-op when no drive/session).
            vaultMirror?.onNoteCreated(id)
        }
    }

    fun deleteNote(note: NoteEntity) {
        viewModelScope.launch {
            // Capture the vault link before the row is gone, so the deletion
            // can be tombstoned in the vault (now or on the next unlock).
            val vaultRecordId = note.vaultRecordId
            noteDao.delete(note)
            vaultMirror?.onRowDeleted(vaultRecordId, VaultSyncPlanner.Kind.NOTE)
        }
    }
}