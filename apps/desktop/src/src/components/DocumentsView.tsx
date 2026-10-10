import { useState, useEffect, useCallback } from 'react';
import { documentSourceSummary } from '../lib/readiness';
import { tauriApi, type DocumentMetadata } from '../lib/tauri';

const TYPE_ICONS: Record<string, string> = {
  PDF: '📄',
  DOCX: '📝',
  TXT: '📃',
  MARKDOWN: '📋',
  CSV: '📊',
  XLSX: '📈',
  PPTX: '📊',
  IMAGE: '🖼️',
  AUDIO: '🎙️',
  WEB_PAGE: '🌐',
};

function formatFileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function DocumentsView() {
  const [documents, setDocuments] = useState<DocumentMetadata[]>([]);
  const [searchQuery, setSearchQuery] = useState('');
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');

  const loadDocuments = useCallback(async () => {
    setLoading(true);
    setError('');
    try {
      const vaultInfo = await tauriApi.detectVault();
      if (vaultInfo.detected) {
        const docs = await tauriApi.listDocuments(vaultInfo.vault_root);
        setDocuments(docs);
      } else {
        setDocuments([]);
      }
    } catch (e) {
      setError(`Failed to load documents: ${e instanceof Error ? e.message : String(e)}`);
      setDocuments([]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadDocuments();
  }, [loadDocuments]);

  const filteredDocs = searchQuery
    ? documents.filter(d =>
        d.title.toLowerCase().includes(searchQuery.toLowerCase()) ||
        d.document_type.toLowerCase().includes(searchQuery.toLowerCase())
      )
    : documents;

  return (
    <div>
      <div className="main-header">
        <h2>Documents</h2>
        <div className="main-header-actions">
          <button className="btn btn-primary btn-sm" onClick={loadDocuments}>
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
              <polyline points="1 4 1 10 7 10" />
              <path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10" />
            </svg>
            Refresh
          </button>
        </div>
      </div>

      {/* Search bar */}
      <div style={{ padding: '0 24px 16px', borderBottom: '1px solid var(--border)' }}>
        <div style={{ display: 'flex', gap: '8px' }}>
          <input
            type="text"
            placeholder="Filter by title or document type…"
            value={searchQuery}
            onChange={e => setSearchQuery(e.target.value)}
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
        {loading ? (
          <div style={{ display: 'flex', justifyContent: 'center', padding: '48px' }}>
            <span className="spinner" />
          </div>
        ) : error ? (
          <div className="empty-state">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <circle cx="12" cy="12" r="10" />
              <line x1="15" y1="9" x2="9" y2="15" />
              <line x1="9" y1="9" x2="15" y2="15" />
            </svg>
            <h3>Error loading documents</h3>
            <p>{error}</p>
          </div>
        ) : filteredDocs.length > 0 ? (
          <div className="memory-grid">
            {filteredDocs.map(doc => (
              <div key={doc.id} className="memory-card">
                <div className="memory-card-type knowledge">
                  {doc.document_type.toLowerCase()}
                </div>
                <div style={{ fontSize: '24px', marginBottom: '8px' }}>
                  {TYPE_ICONS[doc.document_type] || '📄'}
                </div>
                <div className="memory-card-title">{doc.title}</div>
                <div className="memory-card-preview">
                  {formatFileSize(doc.file_size_bytes)} · {doc.source_platform}

                </div>
                <div className="memory-card-preview" style={{ marginTop: '8px', overflowWrap: 'anywhere' }}>
                  {documentSourceSummary(doc)}
                </div>
              </div>
            ))}
          </div>
        ) : (
          <div className="empty-state">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
              <polyline points="14 2 14 8 20 8" />
              <line x1="16" y1="13" x2="8" y2="13" />
              <line x1="16" y1="17" x2="8" y2="17" />
              <polyline points="10 9 9 9 8 9" />
            </svg>
            <h3>No documents yet</h3>
            <p>No documents are listed in the current vault. Chat attachments are separate from this list; attaching a file does not automatically create a retained library source.</p>
          </div>
        )}

        {/* Supported formats info */}
        <div style={{ marginTop: '24px', padding: '16px', background: 'var(--bg-secondary)', border: '1px solid var(--border)', borderRadius: 'var(--radius-md)' }}>
          <h4 style={{ fontSize: '13px', fontWeight: 600, color: 'var(--text-secondary)', marginBottom: '8px' }}>
            📄 Document Processing
          </h4>
          <div style={{ fontSize: '12px', color: 'var(--text-muted)', lineHeight: 1.6 }}>
            <p><strong>Text extraction:</strong> TXT, Markdown, CSV and HTML — bounded extracts, not proof of complete document coverage</p>
            <p><strong>Rich documents:</strong> Text-layer PDF, DOCX, XLSX, PPTX — extracted locally via Rust backend. Legacy DOC/XLS/PPT are not supported by these parsers.</p>
            <p><strong>OCR:</strong> Image text extraction lives in the Accessibility view (Blind Aid / OCR), not in Documents</p>
            <p><strong>PDF limits:</strong> 20 MiB input, 100 pages, 8,000 bytes of excerpt text. Page markers describe this extract, not durable citations. Empty text-layer pages are reported; scanned PDFs have no OCR fallback here.</p>
            <p><strong>Search:</strong> This list filters metadata only. Memory retrieval elsewhere uses lexical TF-IDF, not vector search. Source text is untrusted evidence, not instructions or permission to act.</p>
          </div>
        </div>
      </div>
    </div>
  );
}