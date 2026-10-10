import assert from 'node:assert/strict';
import test from 'node:test';
// Node 24's built-in type stripping executes the production pure helper, no copied logic.
import { modelReadiness, documentSourceSummary, TRUSTED_HOST_DISCLOSURE, CODING_ISOLATION_DISCLOSURE } from '../src/lib/readiness.ts';

const selected = { name: 'Existing model', path: '/existing/model.gguf', available: true };
const value = (rows, label) => rows.find(row => row.label === label).value;
const rows = (model = selected, status = 'NOT_LOADED', runtime = null, cache = null) => modelReadiness(model, status, runtime, cache);

test('presence and selection never become integrity or loaded evidence', () => {
  const result = rows();
  assert.equal(value(result, 'Selection'), 'Existing model');
  assert.equal(value(result, 'Selected asset'), 'Present on disk');
  assert.match(value(result, 'Integrity evidence'), /Unknown/);
  assert.equal(value(result, 'Runtime'), 'Not loaded');
});
test('missing selection remains missing even while another model is running', () => {
  const result = rows({ ...selected, available: false }, 'LOADED', { model_path: '/other.gguf' });
  assert.equal(value(result, 'Selected asset'), 'Missing on disk');
  assert.equal(value(result, 'Runtime'), 'Another or unidentified model loaded');
});
test('no selection and no cache remain unknown, not ready', () => {
  const result = rows(undefined, 'LOADED', null);
  // Explicit call: undefined is significant, not the fixture default.
  assert.equal(value(modelReadiness(undefined, 'NOT_LOADED', null, null), 'Selection'), 'None selected');
  assert.match(value(result, 'Runtime'), /unidentified/);
});
test('a matching source or verified cache path can identify the loaded selection', () => {
  assert.equal(value(rows(selected, 'LOADED', { model_path: selected.path }), 'Runtime'), 'Selected model loaded');
  const cache = { staged: true, cached_path: '/host/sha.gguf' };
  assert.equal(value(rows(selected, 'GENERATING', { model_path: cache.cached_path }, cache), 'Runtime'), 'Selected model loaded');
  assert.equal(value(rows(selected, 'LOADED', { model_path: cache.cached_path }, { ...cache, staged: false }), 'Runtime'), 'Another or unidentified model loaded');
});
test('cache probe and hash verification are distinct; context metadata is not integrity', () => {
  const result = rows({ ...selected, context_verified: true }, 'NOT_LOADED', null, { staged: true });
  assert.equal(value(result, 'Integrity evidence'), 'Verified cache marker present');
  assert.match(result.find(row => row.label === 'Integrity evidence').detail, /not a fresh hash/);
  assert.match(value(rows(selected, 'NOT_LOADED', null, { staged: false }), 'Integrity evidence'), /No verified cache/);
});
test('loading and errors do not inherit loaded badges', () => {
  for (const status of ['LOADING', 'ERROR', 'NOT_LOADED']) {
    assert.notEqual(value(rows(selected, status, { model_path: selected.path }), 'Runtime'), 'Selected model loaded');
  }
});
test('failed status observation is unknown rather than not loaded', () => {
  assert.equal(value(rows(selected, null), 'Runtime'), 'Unknown / status unavailable');
});
test('projector configuration is not vision or speech qualification', () => {
  const result = rows({ ...selected, mmproj_path: '/missing/projector.gguf' });
  assert.equal(value(result, 'Image input'), 'Projector path configured');
  assert.match(result.find(row => row.label === 'Image input').detail, /does not verify/);
  assert.match(result.find(row => row.label === 'Image input').detail, /Speech readiness is separate/);
});
test('helper does not mutate selection, runtime or cache', () => {
  const model = Object.freeze({ ...selected });
  const runtime = Object.freeze({ model_path: '/other.gguf' });
  const cache = Object.freeze({ staged: true, cached_path: '/host/cached.gguf' });
  modelReadiness(model, 'LOADED', runtime, cache);
  assert.equal(runtime.model_path, '/other.gguf');
  assert.equal(model.path, selected.path);
});
test('source metadata preserves zero, handles unknown fields and never asserts provenance verification', () => {
  const summary = documentSourceSummary({ id: 'old-id', source_platform: '', page_count: null, word_count: 0 });
  assert.match(summary, /Source ID: old-id/);
  assert.match(summary, /Recorded platform: unknown/);
  assert.match(summary, /Recorded words: 0/);
  assert.doesNotMatch(summary, /Recorded pages/);
  assert.match(summary, /not proof of complete extraction/);
  const unknown = documentSourceSummary({ id: 'android-id', source_platform: 'ANDROID', page_count: -1, word_count: NaN });
  assert.doesNotMatch(unknown, /Recorded (pages|words)/);
});
test('execution disclosures reflect static confirmation and Windows fail-closed isolation', () => {
  assert.match(TRUSTED_HOST_DISCLOSURE, /static AllowedOnce/);
  assert.match(TRUSTED_HOST_DISCLOSURE, /not an interactive approval/);
  assert.match(TRUSTED_HOST_DISCLOSURE, /partial enforcement/);
  assert.match(TRUSTED_HOST_DISCLOSURE, /no general undo/);
  assert.match(CODING_ISOLATION_DISCLOSURE, /blocked on Windows and macOS/);
  assert.match(CODING_ISOLATION_DISCLOSURE, /no trusted-host fallback/);
});
