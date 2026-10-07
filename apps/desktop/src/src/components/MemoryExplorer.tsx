import { useState, useEffect, useCallback, useRef } from 'react';
import { tauriApi } from '../lib/tauri';

interface Memory {
  id: string;
  type: string;
  title: string;
  preview: string;
  date: string;
  source: string;
}

// Typing settles for this long before a search is issued.
const SEARCH_DEBOUNCE_MS = 300;

export function MemoryExplorer() {
  const [memories, setMemories] = useState<Memory[]>([]);
  // Stage 6 fix of the dead search (`const searchQuery = ''`): the search box
  // drives `searchInput`; after the debounce the trimmed text becomes
  // `searchQuery`, which is sent to `search_memories` (empty = wildcard).
  const [searchInput, setSearchInput] = useState('');
  const [searchQuery, setSearchQuery] = useState('');
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  // Only the newest request may update the list (a slow older search never
  // overwrites a newer one).
  const requestSeq = useRef(0);
  const mounted = useRef(false);

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  useEffect(() => {
    const id = window.setTimeout(() => setSearchQuery(searchInput.trim()), SEARCH_DEBOUNCE_MS);
    return () => window.clearTimeout(id);
  }, [searchInput]);

  const loadMemories = useCallback(async () => {
    const seq = ++requestSeq.current;
    setLoading(true);
    setError('');
    try {
      // Detect vault root from USB pendrive, not hardcoded path
      const vaultInfo = await tauriApi.detectVault();
      const vaultRoot = vaultInfo.detected ? vaultInfo.vault_root : '';

      const result = await tauriApi.searchMemories({
        query: searchQuery || '*',
        memory_types: [],
        limit: 50,
        min_relevance: 0.0,
      }, vaultRoot);
      if (!mounted.current || seq !== requestSeq.current) return;
      setMemories(result.map((m: { id: string; memory_type: string; title: string; preview: string; created_at: string }) => ({
        id: m.id,
        type: m.memory_type.toLowerCase(),
        title: m.title,
        preview: m.preview,
        date: m.created_at ? new Date(m.created_at).toLocaleDateString() : '',
        source: 'Vault',
      })));
    } catch (e) {
      if (!mounted.current || seq !== requestSeq.current) return;
      setError(`Failed to load memories: ${e instanceof Error ? e.message : String(e)}`);
      setMemories([]);
    } finally {
      if (mounted.current && seq === requestSeq.current) setLoading(false);
    }
  }, [searchQuery]);

  useEffect(() => {
    loadMemories();
  }, [loadMemories]);

  return (
    <div>
      <div className="main-header">
        <h2>Memory</h2>
        <div className="main-header-actions">
          <button className="btn btn-secondary btn-sm" onClick={loadMemories}>
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
              <polyline points="1 4 1 10 7 10" />
              <path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10" />
            </svg>
            Refresh
          </button>
        </div>
      </div>

      {/* Search bar (same look as the Documents view's search bar) */}
      <div style={{ padding: '0 24px 16px', borderBottom: '1px solid var(--border)' }}>
        <div style={{ display: 'flex', gap: '8px' }}>
          <input
            type="text"
            placeholder="Search memories…"
            aria-label="Search memories"
            value={searchInput}
            onChange={e => setSearchInput(e.target.value)}
            style={{
              flex: 1,
              padding: '10px 16px',
              background: 'var(--bg-tertiary)',
              border: '1px solid var(--border)',
              borderRadius: 'var(--radius-md)',
              color: 'var(--text-primary)',
              fontSize: '14px',
              outline: 'none',
            }}
          />
        </div>
      </div>

      <div className="main-body">
        {error ? (
          <div className="empty-state">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <circle cx="12" cy="12" r="10" />
              <line x1="15" y1="9" x2="9" y2="15" />
              <line x1="9" y1="9" x2="15" y2="15" />
            </svg>
            <h3>Error loading memories</h3>
            <p>{error}</p>
          </div>
        ) : loading ? (
          <div style={{ display: 'flex', justifyContent: 'center', padding: '48px' }}>
            <span className="spinner" />
          </div>
        ) : memories.length > 0 ? (
          <div className="memory-grid">
            {memories.map(mem => (
              <div key={mem.id} className="memory-card">
                <div className={`memory-card-type ${mem.type}`}>{mem.type}</div>
                <div className="memory-card-title">{mem.title}</div>
                <div className="memory-card-preview">{mem.preview}</div>
                <div style={{ display: 'flex', justifyContent: 'space-between', marginTop: '8px', fontSize: '11px', color: 'var(--text-muted)' }}>
                  <span>{mem.date}</span>
                  <span>{mem.source}</span>
                </div>
              </div>
            ))}
          </div>
        ) : searchQuery ? (
          <div className="empty-state">
            <h3>No memories match</h3>
            <p>Nothing in the vault matches “{searchQuery}”.</p>
          </div>
        ) : (
          <div className="empty-state">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M12 2L2 7l10 5 10-5-10-5z" />
              <path d="M2 17l10 5 10-5" />
              <path d="M2 12l10 5 10-5" />
            </svg>
            <h3>No memories yet</h3>
            <p>Memories will appear here once you start chatting with Gemma 4 or create notes. All memories are encrypted and stored on your Pocket USB.</p>
          </div>
        )}
      </div>
    </div>
  );
}
