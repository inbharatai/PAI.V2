import type { ReadinessRow } from '../lib/readiness';

export function ModelReadinessPanel({ rows, refreshing, onRefresh }: {
  rows: ReadinessRow[];
  refreshing: boolean;
  onRefresh: () => void;
}) {
  return (
    <section className="settings-section" aria-labelledby="model-readiness-title" style={{ marginBottom: '24px' }}>
      <div className="settings-section-header" id="model-readiness-title">Capability readiness — observations only</div>
      <div className="settings-section-body">
        {rows.map(row => (
          <div className="settings-row" key={row.label}>
            <div>
              <div className="settings-row-label">{row.label}: {row.value}</div>
              <div className="settings-row-desc">{row.detail}</div>
            </div>
          </div>
        ))}
        <div style={{ padding: '12px', fontSize: '12px', lineHeight: 1.6 }}>
          <button className="btn btn-secondary btn-sm" disabled={refreshing} onClick={onRefresh}>
            {refreshing ? 'Refreshing observations…' : 'Refresh observations'}
          </button>
          <p>No model download, load, permission request or self-test runs from this checklist.</p>
          <a href="#model-assets">Review local assets</a>{' · '}
          <a href="#model-configuration">Review configuration</a>{' · '}
          <a href="#model-actions">Go to load / health controls</a>
        </div>
      </div>
    </section>
  );
}
