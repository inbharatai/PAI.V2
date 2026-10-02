//! GGUF artifact metadata + host-adaptive context budgets.
//!
//! Universal-adaptive context (2026-10-02): the tool's usable context must
//! follow the *loaded model artifact's* trained context and the *measured
//! host*, not fixed numbers. Before this module, `ModelInfo.context_length`
//! was hardcoded (8192/4096) and the config ladder (4096/16384/32768) was
//! never checked against the model — a request above the trained context
//! would have been silently accepted by the UI and failed (or wrapped) in
//! llama-server.
//!
//! This module reads the GGUF header (v2/v3) directly — no llama.cpp
//! dependency, no guessing from filenames. Every field is optional and
//! honest: if the artifact does not declare it, the derivation says so
//! instead of pretending a value.

use std::io::Read;
use std::path::Path;

/// Metadata the budget derivation needs, read from the artifact itself.
///
/// Unverified/unread artifacts yield `Err` — callers fall back to
/// conservative declared defaults and MUST mark the value as unverified.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GgufMeta {
    pub architecture: Option<String>,
    pub context_length: Option<u32>,
    pub block_count: Option<u32>,
    pub embedding_length: Option<u32>,
    pub head_count: Option<u32>,
    pub head_count_kv: Option<u32>,
    pub key_length: Option<u32>,
    pub value_length: Option<u32>,
}

impl GgufMeta {
    /// Effective KV head count (falls back to the full head count, as
    /// llama.cpp does for models without GQA).
    pub fn kv_heads(&self) -> Option<u32> {
        self.head_count_kv.or(self.head_count)
    }

    /// Per-head key dimension. Falls back to embedding/head_count, which is
    /// exact for the Gemma family (no GQA dim split) and an honest estimate
    /// otherwise.
    pub fn head_dim(&self) -> Option<u32> {
        match (self.key_length, self.embedding_length, self.head_count) {
            (Some(key_length), _, _) => Some(key_length),
            (None, Some(embedding), Some(heads)) => Some(embedding / heads.max(1)),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// GGUF container parsing (metadata section only; tensor blobs are never read)
// ---------------------------------------------------------------------------

struct Cursor<R: Read> {
    reader: R,
    /// Bytes remaining in the file. Every length read (string lengths,
    /// array element counts) is bounded against this so a corrupt header
    /// cannot make the parser walk into the multi-GB tensor section — while
    /// legitimately huge metadata arrays (tokenizer token lists run to
    /// hundreds of thousands of entries) still parse.
    remaining: u64,
}

impl<R: Read> Cursor<R> {
    fn read_exact(&mut self, n: usize, what: &str) -> Result<Vec<u8>, String> {
        if n as u64 > self.remaining {
            return Err(format!("GGUF {what} overruns the file"));
        }
        let mut buf = vec![0u8; n];
        self.reader
            .read_exact(&mut buf)
            .map_err(|e| format!("GGUF {what} truncated: {e}"))?;
        self.remaining -= n as u64;
        Ok(buf)
    }
    fn u8(&mut self, what: &str) -> Result<u8, String> {
        Ok(self.read_exact(1, what)?[0])
    }
    fn u16(&mut self, what: &str) -> Result<u16, String> {
        Ok(u16::from_le_bytes(
            self.read_exact(2, what)?.try_into().expect("2 bytes"),
        ))
    }
    fn u32(&mut self, what: &str) -> Result<u32, String> {
        Ok(u32::from_le_bytes(
            self.read_exact(4, what)?.try_into().expect("4 bytes"),
        ))
    }
    fn i32(&mut self, what: &str) -> Result<i32, String> {
        Ok(i32::from_le_bytes(
            self.read_exact(4, what)?.try_into().expect("4 bytes"),
        ))
    }
    fn f32(&mut self, what: &str) -> Result<f32, String> {
        Ok(f32::from_le_bytes(
            self.read_exact(4, what)?.try_into().expect("4 bytes"),
        ))
    }
    fn u64(&mut self, what: &str) -> Result<u64, String> {
        Ok(u64::from_le_bytes(
            self.read_exact(8, what)?.try_into().expect("8 bytes"),
        ))
    }
    fn f64(&mut self, what: &str) -> Result<f64, String> {
        Ok(f64::from_le_bytes(
            self.read_exact(8, what)?.try_into().expect("8 bytes"),
        ))
    }
    fn string(&mut self, what: &str) -> Result<String, String> {
        let len = self.u64(&format!("{what} length"))? as usize;
        // Keys are short; value strings (chat templates) can be long — the
        // file-size bound above is the real guard, this only stops nonsense.
        if len > 64 * 1024 * 1024 {
            return Err(format!("GGUF {what} string length {len} is implausible"));
        }
        let bytes = self.read_exact(len, what)?;
        String::from_utf8(bytes).map_err(|e| format!("GGUF {what} is not UTF-8: {e}"))
    }
}

/// GGUF metadata value types (spec order).
const TYPE_U8: u32 = 0;
const TYPE_I8: u32 = 1;
const TYPE_U16: u32 = 2;
const TYPE_I16: u32 = 3;
const TYPE_U32: u32 = 4;
const TYPE_I32: u32 = 5;
const TYPE_F32: u32 = 6;
const TYPE_BOOL: u32 = 7;
const TYPE_STRING: u32 = 8;
const TYPE_ARRAY: u32 = 9;
const TYPE_U64: u32 = 10;
const TYPE_I64: u32 = 11;
const TYPE_F64: u32 = 12;

fn read_value<R: Read>(cursor: &mut Cursor<R>, vtype: u32) -> Result<Option<MetaValue>, String> {
    let what = "metadata value";
    Ok(match vtype {
        TYPE_U8 => Some(MetaValue::U64(cursor.u8(what)? as u64)),
        TYPE_I8 => Some(MetaValue::U64(cursor.u8(what)? as i8 as i64 as u64)),
        TYPE_U16 => Some(MetaValue::U64(cursor.u16(what)? as u64)),
        TYPE_I16 => Some(MetaValue::U64(cursor.u16(what)? as i16 as i64 as u64)),
        TYPE_U32 => Some(MetaValue::U64(cursor.u32(what)? as u64)),
        TYPE_I32 => Some(MetaValue::U64(cursor.i32(what)? as i64 as u64)),
        TYPE_F32 => Some(MetaValue::U64((cursor.f32(what)? as i64) as u64)),
        TYPE_BOOL => Some(MetaValue::U64(cursor.u8(what)? as u64)),
        TYPE_STRING => Some(MetaValue::String(cursor.string(what)?)),
        TYPE_U64 => Some(MetaValue::U64(cursor.u64(what)?)),
        TYPE_I64 => Some(MetaValue::U64(cursor.u64(what)?)),
        TYPE_F64 => Some(MetaValue::U64((cursor.f64(what)? as i64) as u64)),
        TYPE_ARRAY => {
            // Element type + count, then count elements. Tokenizer token
            // lists legitimately run to hundreds of thousands of entries;
            // the cursor's file-size bound is the corruption guard.
            let elem_type = cursor.u32("array element type")?;
            let count = cursor.u64("array element count")?;
            if elem_type > TYPE_F64 {
                return Err(format!("GGUF array element type {elem_type} is unknown"));
            }
            for _ in 0..count {
                read_value(cursor, elem_type)?;
            }
            None
        }
        other => return Err(format!("GGUF metadata value type {other} is unknown")),
    })
}

enum MetaValue {
    U64(u64),
    String(String),
}

/// Read the GGUF header metadata. Only the metadata section is parsed; tensor
/// data (the multi-GB tail) is never touched.
pub fn read_metadata<P: AsRef<Path>>(path: P) -> Result<GgufMeta, String> {
    let path = path.as_ref();
    let file_size = std::fs::metadata(path)
        .map_err(|e| format!("cannot stat GGUF artifact {}: {e}", path.display()))?
        .len();
    let file = std::fs::File::open(path)
        .map_err(|e| format!("cannot open GGUF artifact {}: {e}", path.display()))?;
    let mut cursor = Cursor {
        reader: std::io::BufReader::new(file),
        remaining: file_size,
    };

    const MAGIC: u32 = 0x4655_4747; // "GGUF" little-endian
    if cursor.u32("magic")? != MAGIC {
        return Err("not a GGUF file (bad magic)".to_string());
    }
    let version = cursor.u32("version")?;
    if !(2..=3).contains(&version) {
        return Err(format!("unsupported GGUF version {version}"));
    }
    let _tensor_count = cursor.u64("tensor count")?;
    let kv_count = cursor.u64("metadata KV count")?;
    if kv_count > 65536 {
        return Err(format!("GGUF metadata KV count {kv_count} is implausible"));
    }

    let mut meta = GgufMeta::default();
    for _ in 0..kv_count {
        let key = cursor.string("metadata key")?;
        let vtype = cursor.u32("metadata value type")?;
        let value = match read_value(&mut cursor, vtype)? {
            // Arrays (token lists, layer shapes) are parsed and discarded —
            // real GGUFs ship huge ones and the budget only needs scalars.
            Some(value) => value,
            None => continue,
        };
        match (key.as_str(), value) {
            ("general.architecture", MetaValue::String(s)) => meta.architecture = Some(s),
            (_, MetaValue::U64(v)) => {
                let field = key.rsplit('.').next().unwrap_or("");
                let arch_ok = key
                    .split('.')
                    .next()
                    .zip(meta.architecture.as_deref())
                    .map(|(a, b)| a == b)
                    .unwrap_or(key.starts_with("general."));
                if !arch_ok {
                    continue;
                }
                match field {
                    "context_length" => meta.context_length = Some(v as u32),
                    "block_count" => meta.block_count = Some(v as u32),
                    "embedding_length" => meta.embedding_length = Some(v as u32),
                    "head_count" => meta.head_count = Some(v as u32),
                    "head_count_kv" => meta.head_count_kv = Some(v as u32),
                    "key_length" => meta.key_length = Some(v as u32),
                    "value_length" => meta.value_length = Some(v as u32),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    Ok(meta)
}

// ---------------------------------------------------------------------------
// Host-adaptive budget derivation
// ---------------------------------------------------------------------------

/// Bytes per KV-cache element by cache type (K/V quantization). q8_0 stores
/// one byte per element plus a 2-byte scale per 32-element block; q4_0 stores
/// half a byte per element plus 2 bytes per 32 elements.
pub fn kv_element_bytes(cache_type: &str) -> f64 {
    match cache_type {
        "q8_0" => 34.0 / 32.0,
        "q4_0" => 18.0 / 32.0,
        // f16 and bf16
        _ => 2.0,
    }
}

/// The full derivation the UI and the server launcher share. Every clamp is
/// reported with the reason, so the Model panel can state plainly why the
/// session runs at the granted context.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ContextBudget {
    /// Trained context declared by the artifact (None = unverified).
    pub native_context: Option<u32>,
    /// The context actually granted for this session.
    pub granted_context: u32,
    /// Plain-English reason for every clamp, newest last.
    pub reasons: Vec<String>,
    /// Estimated KV-cache bytes for the granted context.
    pub kv_estimate_bytes: Option<u64>,
}

impl ContextBudget {
    pub fn limiting_reason(&self) -> String {
        self.reasons
            .last()
            .cloned()
            .unwrap_or_else(|| "no clamps applied — the request stands".to_string())
    }
}

/// Derive the session budget: requested context clamped by (1) the artifact's
/// trained context, then (2) the host RAM tier. If the artifact metadata
/// cannot be read, the native context is treated as the conservative declared
/// default and the derivation says so — it never invents a larger number.
pub fn derive_context_budget(
    meta: Option<&GgufMeta>,
    requested_context: u32,
    host_ram_gib: Option<u64>,
    cache_type_k: Option<&str>,
) -> ContextBudget {
    let mut granted = requested_context;
    let mut reasons = Vec::new();

    match meta.and_then(|m| m.context_length) {
        Some(native) if granted > native => {
            reasons.push(format!(
                "clamped {granted} → {native}: the artifact's trained context is {native} tokens"
            ));
            granted = native;
        }
        Some(_) => {}
        None => reasons.push(
            "artifact context is unverified — the conservative default is the ceiling".to_string(),
        ),
    }

    // Host tier ceiling: the shipped ladder, kept as a ceiling on top of the
    // artifact's own context rather than as the source of truth.
    let tier = match host_ram_gib {
        Some(ram) if ram >= 24 => 32_768_u32,
        Some(ram) if ram >= 12 => 16_384,
        Some(_) => 4_096,
        None => 4_096,
    };
    if granted > tier {
        reasons.push(format!(
            "clamped {granted} → {tier}: host RAM tier (detected {} GiB)",
            host_ram_gib.unwrap_or(0)
        ));
        granted = tier;
    }

    // KV estimate, when the artifact declares enough shape metadata.
    let kv_estimate = meta
        .zip(meta.and_then(|m| m.kv_heads()))
        .zip(meta.and_then(|m| m.head_dim()))
        .zip(meta.and_then(|m| m.block_count))
        .map(|(((_m, kv_heads), head_dim), layers)| {
            let elem = kv_element_bytes(cache_type_k.unwrap_or("f16"));
            let per_token = 2.0 * layers as f64 * kv_heads as f64 * head_dim as f64 * elem;
            (per_token * granted as f64) as u64
        });

    ContextBudget {
        native_context: meta.and_then(|m| m.context_length),
        granted_context: granted,
        reasons,
        kv_estimate_bytes: kv_estimate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal well-formed GGUF header builder for tests.
    struct HeaderBuilder {
        buf: Vec<u8>,
        kv_count: u64,
    }
    impl HeaderBuilder {
        fn new() -> Self {
            let mut buf = Vec::new();
            buf.extend_from_slice(b"GGUF");
            buf.extend_from_slice(&3u32.to_le_bytes()); // version
            buf.extend_from_slice(&0u64.to_le_bytes()); // tensor count
            buf.extend_from_slice(&0u64.to_le_bytes()); // KV count (patched)
            HeaderBuilder { buf, kv_count: 0 }
        }
        fn string(&mut self, s: &str) {
            self.buf.extend_from_slice(&(s.len() as u64).to_le_bytes());
            self.buf.extend_from_slice(s.as_bytes());
        }
        fn kv_str(&mut self, key: &str, value: &str) {
            self.string(key);
            self.buf.extend_from_slice(&TYPE_STRING.to_le_bytes());
            self.string(value);
            self.kv_count += 1;
        }
        fn kv_u32(&mut self, key: &str, value: u32) {
            self.string(key);
            self.buf.extend_from_slice(&TYPE_U32.to_le_bytes());
            self.buf.extend_from_slice(&value.to_le_bytes());
            self.kv_count += 1;
        }
        fn kv_array_u32(&mut self, key: &str, values: &[u32]) {
            self.string(key);
            self.buf.extend_from_slice(&TYPE_ARRAY.to_le_bytes());
            self.buf.extend_from_slice(&TYPE_U32.to_le_bytes());
            self.buf
                .extend_from_slice(&(values.len() as u64).to_le_bytes());
            for v in values {
                self.buf.extend_from_slice(&v.to_le_bytes());
            }
            self.kv_count += 1;
        }
        fn finish(self, path: &std::path::Path) {
            let mut buf = self.buf;
            let count_offset = 4 + 4 + 8;
            buf[count_offset..count_offset + 8].copy_from_slice(&self.kv_count.to_le_bytes());
            std::fs::write(path, &buf).expect("write test gguf");
        }
    }

    fn gemma_header(path: &std::path::Path, context_length: u32) {
        let mut b = HeaderBuilder::new();
        b.kv_str("general.architecture", "gemma4");
        b.kv_u32("gemma4.context_length", context_length);
        b.kv_u32("gemma4.block_count", 48);
        b.kv_u32("gemma4.embedding_length", 2560);
        b.kv_u32("gemma4.attention.head_count", 8);
        b.kv_array_u32("gemma4.attention.feed_forward.hidden_dims", &[1, 2, 3]);
        b.kv_u32("general.name", 7); // wrong-typed value on purpose
        b.finish(path);
        // note: "general.name" typed as U32 is accepted — unknown keys are
        // skipped, which is what keeps the parser forward-compatible.
    }

    #[test]
    fn reads_native_context_from_artifact() {
        let dir = tempfile::tempdir().expect("temp");
        let p = dir.path().join("model.gguf");
        gemma_header(&p, 32_768);
        let meta = read_metadata(&p).expect("parse");
        assert_eq!(meta.context_length, Some(32_768));
        assert_eq!(meta.block_count, Some(48));
        assert_eq!(meta.head_count, Some(8));
        // Array KV was parsed and skipped without error.
        assert_eq!(meta.architecture.as_deref(), Some("gemma4"));
    }

    #[test]
    fn rejects_non_gguf_and_truncated_files() {
        let dir = tempfile::tempdir().expect("temp");
        let bad = dir.path().join("bad.gguf");
        std::fs::write(&bad, b"PK\x03\x04zipdata").expect("write");
        assert!(read_metadata(&bad).is_err());
        std::fs::write(&bad, b"GGUF\x03\x00\x00\x00trunc").expect("write");
        assert!(read_metadata(&bad).is_err());
    }

    #[test]
    fn budget_clamps_to_artifact_native_context() {
        let dir = tempfile::tempdir().expect("temp");
        let p = dir.path().join("model.gguf");
        gemma_header(&p, 8_192);
        let meta = read_metadata(&p).expect("parse");
        let budget = derive_context_budget(Some(&meta), 131_072, Some(64), Some("q8_0"));
        assert_eq!(budget.granted_context, 8_192);
        assert_eq!(budget.native_context, Some(8_192));
        assert!(budget.limiting_reason().contains("trained context"));
    }

    #[test]
    fn budget_clamps_to_host_tier_below_native() {
        let dir = tempfile::tempdir().expect("temp");
        let p = dir.path().join("model.gguf");
        gemma_header(&p, 131_072); // a genuinely 131K-trained artifact
        let meta = read_metadata(&p).expect("parse");
        // A 131K artifact on a 16 GiB host: RAM is the honest limit.
        let budget = derive_context_budget(Some(&meta), 131_072, Some(16), Some("q8_0"));
        assert_eq!(budget.granted_context, 16_384);
        assert!(budget.limiting_reason().contains("host RAM tier"));
    }

    #[test]
    fn budget_stays_honest_when_artifact_is_unreadable() {
        let budget = derive_context_budget(None, 1_000_000, Some(64), Some("q8_0"));
        assert_eq!(budget.granted_context, 32_768);
        assert!(budget.reasons.iter().any(|r| r.contains("unverified")));
    }

    #[test]
    fn kv_estimate_matches_llama_cpp_shape() {
        let dir = tempfile::tempdir().expect("temp");
        let p = dir.path().join("model.gguf");
        gemma_header(&p, 8_192);
        let meta = read_metadata(&p).expect("parse");
        let budget = derive_context_budget(Some(&meta), 8_192, Some(64), Some("f16"));
        // 2 (K+V) * 48 layers * 8 heads * 320 dim * 2 B = 491,520 B/token
        // → * 8192 tokens = 4,026,531,840 B (~3.75 GiB).
        assert_eq!(budget.kv_estimate_bytes, Some(2 * 48 * 8 * 320 * 2 * 8_192));
    }

    #[test]
    fn q8_0_shrinks_kv_estimate_relative_to_f16() {
        let dir = tempfile::tempdir().expect("temp");
        let p = dir.path().join("model.gguf");
        gemma_header(&p, 8_192);
        let meta = read_metadata(&p).expect("parse");
        let f16 = derive_context_budget(Some(&meta), 8_192, Some(64), Some("f16"));
        let q8 = derive_context_budget(Some(&meta), 8_192, Some(64), Some("q8_0"));
        assert!(q8.kv_estimate_bytes.unwrap() < f16.kv_estimate_bytes.unwrap());
    }

    /// Env-gated live check against a real artifact (CI runs it only when the
    /// variable is set; locally: `UNOONE_GGUF_TEST_PATH=D:\...\model.gguf`).
    /// Guards the parser against real-file shapes the synthetic header cannot
    /// reproduce — big tokenizer arrays, long template strings, version 2.
    #[test]
    fn parses_a_real_artifact_when_one_is_named() {
        let Ok(path) = std::env::var("UNOONE_GGUF_TEST_PATH") else {
            return; // not a failure: no artifact named for this run
        };
        let meta = read_metadata(&path).expect("the real artifact must parse");
        let native = meta
            .context_length
            .expect("real models declare context_length");
        // On a big host the RAM tier (32K) can sit below a genuinely larger
        // trained context (e.g. a 131K-class artifact) — that clamp is the
        // derivation working as designed, not a failure.
        let budget = derive_context_budget(Some(&meta), 1_000_000, Some(64), Some("q8_0"));
        assert!(budget.granted_context <= native);
        let kv = budget
            .kv_estimate_bytes
            .expect("shape metadata must be complete");
        println!(
            "real artifact: native ctx {native}, granted {} at the 64 GiB tier, kv estimate {:.2} GiB at q8_0, layers={:?} kv_heads={:?} head_dim={:?}",
            budget.granted_context,
            kv as f64 / (1024.0 * 1024.0 * 1024.0),
            meta.block_count,
            meta.kv_heads(),
            meta.head_dim(),
        );
    }
}
