// UnoOne Power — Desktop Document Processing
// Lists real documents from vault; text extraction for TXT, MD, CSV, HTML
// PDF and DOCX support added via lopdf and zip-based extraction.
// Search uses TF-IDF relevance scoring instead of fake relevance=0.5.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use unoone_vault_core::{EncryptedRecord, RecordType, Vault};

/// Supported document types (mirrors core-contracts Document.kt)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DocumentType {
    Pdf,
    Docx,
    Txt,
    Markdown,
    Csv,
    Xlsx,
    Pptx,
    Image,
    Audio,
    WebPage,
}

/// Document metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentMetadata {
    pub id: String,
    pub title: String,
    pub document_type: DocumentType,
    pub file_path: String,
    pub file_size_bytes: u64,
    pub created_at: String,
    pub modified_at: String,
    pub source_platform: String,
    pub tags: Vec<String>,
    pub page_count: Option<u32>,
    pub word_count: Option<u32>,
}

/// Document processing result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentProcessResult {
    pub document_id: String,
    pub success: bool,
    pub extracted_text: Option<String>,
    pub summary: Option<String>,
    pub error: Option<String>,
    pub processing_time_ms: u64,
}

/// Memory search query
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySearchQuery {
    pub query: String,
    pub memory_types: Vec<String>,
    pub limit: u32,
    pub min_relevance: f32,
}

/// Memory search result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySearchResult {
    pub id: String,
    pub memory_type: String,
    pub title: String,
    pub preview: String,
    pub relevance: f32,
    pub created_at: String,
}

/// Document processor — reads real files from vault
pub struct DocumentProcessor {
    vault_root: String,
}

impl DocumentProcessor {
    pub fn new(vault_root: &str) -> Self {
        Self {
            vault_root: vault_root.to_string(),
        }
    }

    /// List all documents in the vault directory
    pub fn list_documents(&self) -> Vec<DocumentMetadata> {
        let docs_dir = PathBuf::from(&self.vault_root)
            .join("VAULT")
            .join("documents");

        let mut documents = Vec::new();

        if !docs_dir.exists() {
            return documents;
        }

        if let Ok(entries) = std::fs::read_dir(&docs_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(ext) = path.extension() {
                    let doc_type = match ext.to_string_lossy().to_lowercase().as_str() {
                        "pdf" => DocumentType::Pdf,
                        "docx" | "doc" => DocumentType::Docx,
                        "txt" => DocumentType::Txt,
                        "md" | "markdown" => DocumentType::Markdown,
                        "csv" => DocumentType::Csv,
                        "xlsx" | "xls" => DocumentType::Xlsx,
                        "pptx" | "ppt" => DocumentType::Pptx,
                        "png" | "jpg" | "jpeg" | "gif" | "webp" => DocumentType::Image,
                        "mp3" | "wav" | "ogg" | "flac" => DocumentType::Audio,
                        "html" | "htm" => DocumentType::WebPage,
                        _ => continue,
                    };

                    let file_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

                    let created = std::fs::metadata(&path)
                        .ok()
                        .and_then(|m| m.created().ok())
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs().to_string())
                        .unwrap_or_default();

                    let modified = std::fs::metadata(&path)
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs().to_string())
                        .unwrap_or_default();

                    // Count words for text-based formats
                    let word_count = match doc_type {
                        DocumentType::Txt
                        | DocumentType::Markdown
                        | DocumentType::Csv
                        | DocumentType::WebPage => std::fs::read_to_string(&path)
                            .map(|s| s.split_whitespace().count() as u32)
                            .ok(),
                        _ => None,
                    };

                    documents.push(DocumentMetadata {
                        id: path
                            .file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string(),
                        title: path
                            .file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string(),
                        document_type: doc_type,
                        file_path: path.to_string_lossy().to_string(),
                        file_size_bytes: file_size,
                        created_at: created,
                        modified_at: modified,
                        source_platform: "DESKTOP".to_string(),
                        tags: Vec::new(),
                        page_count: None,
                        word_count,
                    });
                }
            }
        }

        documents
    }

    /// Process a document — extract text from supported formats
    /// TXT, Markdown, CSV, and HTML are fully supported.
    /// PDF uses lopdf for basic text extraction.
    /// DOCX uses zip + XML stripping for basic text extraction.
    pub fn process_document(&self, document_id: &str) -> DocumentProcessResult {
        let start = std::time::Instant::now();

        let docs_dir = PathBuf::from(&self.vault_root)
            .join("VAULT")
            .join("documents");

        // Find the document by ID (filename stem)
        if let Ok(entries) = std::fs::read_dir(&docs_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(stem) = path.file_stem() {
                    if stem.to_string_lossy() == document_id {
                        if let Some(ext) = path.extension() {
                            let ext_str = ext.to_string_lossy().to_lowercase();
                            match ext_str.as_str() {
                                "txt" | "md" | "markdown" => {
                                    // Text formats — read directly
                                    match std::fs::read_to_string(&path) {
                                        Ok(text) => {
                                            let word_count = text.split_whitespace().count();
                                            return DocumentProcessResult {
                                                document_id: document_id.to_string(),
                                                success: true,
                                                extracted_text: Some(text),
                                                summary: Some(format!(
                                                    "[Auto-extracted text, {} words]",
                                                    word_count
                                                )),
                                                error: None,
                                                processing_time_ms: start.elapsed().as_millis()
                                                    as u64,
                                            };
                                        }
                                        Err(e) => {
                                            return DocumentProcessResult {
                                                document_id: document_id.to_string(),
                                                success: false,
                                                extracted_text: None,
                                                summary: None,
                                                error: Some(format!("Failed to read file: {}", e)),
                                                processing_time_ms: start.elapsed().as_millis()
                                                    as u64,
                                            };
                                        }
                                    }
                                }
                                "csv" => {
                                    // CSV — read and format as structured text
                                    match std::fs::read_to_string(&path) {
                                        Ok(text) => {
                                            let lines: Vec<&str> = text.lines().collect();
                                            let row_count = lines.len();
                                            let word_count = text.split_whitespace().count();
                                            // Format CSV as readable text with row markers
                                            let formatted: String = if lines.len() <= 200 {
                                                lines
                                                    .iter()
                                                    .enumerate()
                                                    .map(|(i, line)| {
                                                        format!("Row {}: {}", i + 1, line)
                                                    })
                                                    .collect::<Vec<_>>()
                                                    .join("\n")
                                            } else {
                                                let header = lines
                                                    .first()
                                                    .map(|l| format!("Header: {}", l))
                                                    .unwrap_or_default();
                                                let preview: Vec<String> = lines[1..20]
                                                    .iter()
                                                    .enumerate()
                                                    .map(|(i, line)| {
                                                        format!("Row {}: {}", i + 2, line)
                                                    })
                                                    .collect();
                                                format!(
                                                    "{}\n{}\n... [{} rows total, {} words]",
                                                    header,
                                                    preview.join("\n"),
                                                    row_count,
                                                    word_count
                                                )
                                            };
                                            return DocumentProcessResult {
                                                document_id: document_id.to_string(),
                                                success: true,
                                                extracted_text: Some(formatted),
                                                summary: Some(format!(
                                                    "[CSV: {} rows, {} words]",
                                                    row_count, word_count
                                                )),
                                                error: None,
                                                processing_time_ms: start.elapsed().as_millis()
                                                    as u64,
                                            };
                                        }
                                        Err(e) => {
                                            return DocumentProcessResult {
                                                document_id: document_id.to_string(),
                                                success: false,
                                                extracted_text: None,
                                                summary: None,
                                                error: Some(format!("Failed to read CSV: {}", e)),
                                                processing_time_ms: start.elapsed().as_millis()
                                                    as u64,
                                            };
                                        }
                                    }
                                }
                                "html" | "htm" => {
                                    // HTML — strip tags and extract text content
                                    match std::fs::read_to_string(&path) {
                                        Ok(text) => {
                                            let plain = strip_html_tags(&text);
                                            let word_count = plain.split_whitespace().count();
                                            return DocumentProcessResult {
                                                document_id: document_id.to_string(),
                                                success: true,
                                                extracted_text: Some(plain),
                                                summary: Some(format!(
                                                    "[HTML extracted, {} words]",
                                                    word_count
                                                )),
                                                error: None,
                                                processing_time_ms: start.elapsed().as_millis()
                                                    as u64,
                                            };
                                        }
                                        Err(e) => {
                                            return DocumentProcessResult {
                                                document_id: document_id.to_string(),
                                                success: false,
                                                extracted_text: None,
                                                summary: None,
                                                error: Some(format!("Failed to read HTML: {}", e)),
                                                processing_time_ms: start.elapsed().as_millis()
                                                    as u64,
                                            };
                                        }
                                    }
                                }
                                "pdf" => {
                                    // PDF — use lopdf for basic text extraction
                                    match extract_pdf_text(&path) {
                                        Ok(text) => {
                                            let word_count = text.split_whitespace().count();
                                            return DocumentProcessResult {
                                                document_id: document_id.to_string(),
                                                success: true,
                                                extracted_text: Some(text),
                                                summary: Some(format!(
                                                    "[PDF text-layer excerpt, {} words including extraction notices; not a full-document word count]",
                                                    word_count
                                                )),
                                                error: None,
                                                processing_time_ms: start.elapsed().as_millis()
                                                    as u64,
                                            };
                                        }
                                        Err(e) => {
                                            return DocumentProcessResult {
                                                document_id: document_id.to_string(),
                                                success: false,
                                                extracted_text: None,
                                                summary: None,
                                                error: Some(format!(
                                                    "Failed to extract PDF text: {}",
                                                    e
                                                )),
                                                processing_time_ms: start.elapsed().as_millis()
                                                    as u64,
                                            };
                                        }
                                    }
                                }
                                "docx" => {
                                    // DOCX — extract text from word/document.xml inside the zip
                                    match extract_docx_text(&path) {
                                        Ok(text) => {
                                            let word_count = text.split_whitespace().count();
                                            return DocumentProcessResult {
                                                document_id: document_id.to_string(),
                                                success: true,
                                                extracted_text: Some(text),
                                                summary: Some(format!(
                                                    "[DOCX extracted, {} words]",
                                                    word_count
                                                )),
                                                error: None,
                                                processing_time_ms: start.elapsed().as_millis()
                                                    as u64,
                                            };
                                        }
                                        Err(e) => {
                                            return DocumentProcessResult {
                                                document_id: document_id.to_string(),
                                                success: false,
                                                extracted_text: None,
                                                summary: None,
                                                error: Some(format!(
                                                    "Failed to extract DOCX text: {}",
                                                    e
                                                )),
                                                processing_time_ms: start.elapsed().as_millis()
                                                    as u64,
                                            };
                                        }
                                    }
                                }
                                "xlsx" | "xls" => {
                                    // XLSX — extract cell data from ZIP+XML
                                    match extract_xlsx_text(&path) {
                                        Ok(text) => {
                                            let word_count = text.split_whitespace().count();
                                            return DocumentProcessResult {
                                                document_id: document_id.to_string(),
                                                success: true,
                                                extracted_text: Some(text),
                                                summary: Some(format!(
                                                    "[XLSX extracted, {} words]",
                                                    word_count
                                                )),
                                                error: None,
                                                processing_time_ms: start.elapsed().as_millis()
                                                    as u64,
                                            };
                                        }
                                        Err(e) => {
                                            return DocumentProcessResult {
                                                document_id: document_id.to_string(),
                                                success: false,
                                                extracted_text: None,
                                                summary: None,
                                                error: Some(format!(
                                                    "Failed to extract XLSX text: {}",
                                                    e
                                                )),
                                                processing_time_ms: start.elapsed().as_millis()
                                                    as u64,
                                            };
                                        }
                                    }
                                }
                                "pptx" | "ppt" => {
                                    // PPTX — extract slide text from ZIP+XML
                                    match extract_pptx_text(&path) {
                                        Ok(text) => {
                                            let word_count = text.split_whitespace().count();
                                            return DocumentProcessResult {
                                                document_id: document_id.to_string(),
                                                success: true,
                                                extracted_text: Some(text),
                                                summary: Some(format!(
                                                    "[PPTX extracted, {} words]",
                                                    word_count
                                                )),
                                                error: None,
                                                processing_time_ms: start.elapsed().as_millis()
                                                    as u64,
                                            };
                                        }
                                        Err(e) => {
                                            return DocumentProcessResult {
                                                document_id: document_id.to_string(),
                                                success: false,
                                                extracted_text: None,
                                                summary: None,
                                                error: Some(format!(
                                                    "Failed to extract PPTX text: {}",
                                                    e
                                                )),
                                                processing_time_ms: start.elapsed().as_millis()
                                                    as u64,
                                            };
                                        }
                                    }
                                }
                                _ => {
                                    return DocumentProcessResult {
                                        document_id: document_id.to_string(),
                                        success: false,
                                        extracted_text: None,
                                        summary: None,
                                        error: Some(format!(
                                            "Document format .{} is not yet supported. Supported: .txt, .md, .csv, .html, .pdf, .docx, .xlsx, .pptx",
                                            ext_str
                                        )),
                                        processing_time_ms: start.elapsed().as_millis() as u64,
                                    };
                                }
                            }
                        }
                    }
                }
            }
        }

        DocumentProcessResult {
            document_id: document_id.to_string(),
            success: false,
            extracted_text: None,
            summary: None,
            error: Some(format!("Document '{}' not found in vault", document_id)),
            processing_time_ms: start.elapsed().as_millis() as u64,
        }
    }

    /// Search memories using TF-IDF relevance scoring
    /// Returns results sorted by relevance (highest first), filtered by min_relevance.
    pub fn search_memories(&self, query: &MemorySearchQuery) -> Vec<MemorySearchResult> {
        let memory_dir = PathBuf::from(&self.vault_root).join("VAULT").join("memory");

        if !memory_dir.exists() {
            return Vec::new();
        }

        // Collect all memory files with their content
        let mut file_contents: Vec<(String, String, String, String, String)> = Vec::new(); // (id, title, content, extension, modified_at)
        if let Ok(entries) = std::fs::read_dir(&memory_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .extension()
                    .map(|e| e == "json" || e == "txt" || e == "md")
                    .unwrap_or(false)
                {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        let file_stem = path
                            .file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string();
                        let ext = path
                            .extension()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string();
                        let modified = std::fs::metadata(&path)
                            .ok()
                            .and_then(|m| m.modified().ok())
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs().to_string())
                            .unwrap_or_default();
                        file_contents.push((file_stem.clone(), file_stem, content, ext, modified));
                    }
                }
            }
        }

        score_with_tfidf(query, file_contents, "")
    }
}

/// Tokenize text into lowercase terms for TF-IDF
fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .filter(|s| !s.is_empty() && s.len() > 1) // Skip single-char tokens
        .map(|s| s.to_lowercase())
        .collect()
}

/// Strip HTML tags to extract plain text content
pub(crate) fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut in_script = false;

    for ch in html.chars() {
        match ch {
            '<' => {
                in_tag = true;
                // Check if this is a script or style tag
                let rest = &html[html.find('<').unwrap_or(0)..];
                if rest.starts_with("<script") || rest.starts_with("<style") {
                    in_script = true;
                }
            }
            '>' => {
                in_tag = false;
                result.push(' ');
            }
            _ if !in_tag && !in_script => {
                result.push(ch);
            }
            _ if in_tag && ch == '/' => {
                // Check for closing script/style tags
                // Peek ahead for /script or /style
                // Simplified: just skip
            }
            _ => {} // Skip content inside script/style
        }
    }

    // Collapse whitespace
    let text = result.split_whitespace().collect::<Vec<_>>().join(" ");

    // Truncate very long documents for the agent context window
    unoone_text::truncate_bytes_with_notice(&text, 8000)
}

// Kept separate so the production extractor and hermetic fixtures can be tested
// without compiling the desktop shell or invoking any model.
#[path = "documents_pdf.rs"]
mod pdf_text;
pub(crate) use pdf_text::extract_pdf_text;

/// Extract text from a DOCX file using proper ZIP+XML parsing
pub(crate) fn extract_docx_text(path: &PathBuf) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("Failed to open DOCX: {}", e))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("Failed to read DOCX as ZIP: {}", e))?;

    // Find word/document.xml in the ZIP
    let doc_xml = match archive.by_name("word/document.xml") {
        Ok(mut file) => {
            let mut content = String::new();
            file.read_to_string(&mut content)
                .map_err(|e| format!("Failed to read document.xml: {}", e))?;
            content
        }
        Err(_) => return Err("word/document.xml not found in DOCX archive".to_string()),
    };

    // Parse XML and extract text from <w:t> elements
    let text = extract_text_from_docx_xml(&doc_xml);

    if text.trim().is_empty() {
        return Err("No text could be extracted from this DOCX file.".to_string());
    }

    Ok(unoone_text::truncate_bytes_with_notice(&text, 8000))
}

/// Parse DOCX XML and extract text from <w:t> elements using quick-xml
fn extract_text_from_docx_xml(xml: &str) -> String {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(xml);
    let mut text_parts: Vec<String> = Vec::new();
    let mut in_wt = false;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let local_name = e.local_name();
                if local_name.as_ref() == b"t" {
                    in_wt = true;
                }
            }
            Ok(Event::Text(ref e)) => {
                if in_wt {
                    if let Ok(text) = e.unescape() {
                        text_parts.push(text.to_string());
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                if e.local_name().as_ref() == b"t" {
                    in_wt = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    text_parts.join(" ")
}

/// Extract text from an XLSX file using ZIP+XML parsing
fn extract_xlsx_text(path: &PathBuf) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("Failed to open XLSX: {}", e))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("Failed to read XLSX as ZIP: {}", e))?;

    // Load the shared strings table
    let mut shared_strings: Vec<String> = Vec::new();
    if let Ok(mut ss_file) = archive.by_name("xl/sharedStrings.xml") {
        let mut content = String::new();
        ss_file
            .read_to_string(&mut content)
            .map_err(|e| format!("Failed to read sharedStrings.xml: {}", e))?;
        shared_strings = parse_xlsx_shared_strings(&content);
    }

    // Extract cell data from each worksheet
    let mut all_text = Vec::new();
    let mut sheet_idx = 1;
    loop {
        let sheet_name = format!("xl/worksheets/sheet{}.xml", sheet_idx);
        match archive.by_name(&sheet_name) {
            Ok(mut sheet_file) => {
                let mut content = String::new();
                sheet_file
                    .read_to_string(&mut content)
                    .map_err(|e| format!("Failed to read {}: {}", sheet_name, e))?;
                let sheet_text = parse_xlsx_sheet(&content, &shared_strings);
                if !sheet_text.trim().is_empty() {
                    all_text.push(format!("--- Sheet {} ---\n{}", sheet_idx, sheet_text));
                }
                sheet_idx += 1;
            }
            Err(_) => break,
        }
    }

    if all_text.is_empty() {
        return Err("No text could be extracted from this XLSX file.".to_string());
    }
    Ok(all_text.join("\n\n"))
}

/// Parse XLSX shared strings XML
fn parse_xlsx_shared_strings(xml: &str) -> Vec<String> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(xml);
    let mut strings = Vec::new();
    let mut in_si = false;
    let mut in_t = false;
    let mut current = String::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                if e.local_name().as_ref() == b"si" {
                    in_si = true;
                    current.clear();
                }
                if e.local_name().as_ref() == b"t" {
                    in_t = true;
                }
            }
            Ok(Event::End(ref e)) => {
                if e.local_name().as_ref() == b"si" {
                    in_si = false;
                    strings.push(current.trim().to_string());
                }
                if e.local_name().as_ref() == b"t" {
                    in_t = false;
                }
            }
            Ok(Event::Text(ref e)) => {
                if in_t && in_si {
                    if let Ok(text) = e.unescape() {
                        current.push_str(&text);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    strings
}

/// Parse an XLSX worksheet and return cell data as tab-separated rows
fn parse_xlsx_sheet(xml: &str, shared_strings: &[String]) -> String {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(xml);
    let mut rows: Vec<String> = Vec::new();
    let mut current_row = String::new();
    let mut in_value = false;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                if e.local_name().as_ref() == b"v" {
                    in_value = true;
                }
            }
            Ok(Event::End(ref e)) => {
                if e.local_name().as_ref() == b"v" {
                    in_value = false;
                }
                if e.local_name().as_ref() == b"row" {
                    if !current_row.is_empty() {
                        rows.push(current_row.clone());
                    }
                    current_row.clear();
                }
            }
            Ok(Event::Text(ref e)) => {
                if in_value {
                    if let Ok(text) = e.unescape() {
                        let idx: usize = text.parse().unwrap_or(0);
                        let cell_text = if idx < shared_strings.len() {
                            shared_strings[idx].clone()
                        } else {
                            text.to_string()
                        };
                        if !current_row.is_empty() {
                            current_row.push('\t');
                        }
                        current_row.push_str(&cell_text);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    rows.join("\n")
}

/// Extract text from a PPTX file — slide text from <a:t> elements
fn extract_pptx_text(path: &PathBuf) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("Failed to open PPTX: {}", e))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("Failed to read PPTX as ZIP: {}", e))?;

    let mut slides = Vec::new();
    let mut slide_idx = 1;

    loop {
        let slide_name = format!("ppt/slides/slide{}.xml", slide_idx);
        match archive.by_name(&slide_name) {
            Ok(mut slide_file) => {
                let mut content = String::new();
                slide_file
                    .read_to_string(&mut content)
                    .map_err(|e| format!("Failed to read {}: {}", slide_name, e))?;
                let slide_text = parse_pptx_slide(&content);
                if !slide_text.trim().is_empty() {
                    slides.push(format!("--- Slide {} ---\n{}", slide_idx, slide_text));
                }
                slide_idx += 1;
            }
            Err(_) => break,
        }
    }

    if slides.is_empty() {
        return Err("No text could be extracted from this PPTX file.".to_string());
    }
    Ok(slides.join("\n\n"))
}

/// Parse a PPTX slide XML and extract text from <a:t> elements
fn parse_pptx_slide(xml: &str) -> String {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(xml);
    let mut text_parts: Vec<String> = Vec::new();
    let mut in_at = false;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                if e.local_name().as_ref() == b"t" {
                    in_at = true;
                }
            }
            Ok(Event::End(ref e)) => {
                if e.local_name().as_ref() == b"t" {
                    in_at = false;
                }
            }
            Ok(Event::Text(ref e)) => {
                if in_at {
                    if let Ok(text) = e.unescape() {
                        text_parts.push(text.to_string());
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    text_parts.join(" ")
}

// ---------------------------------------------------------------------------
// Migrated-record read path (Wave 3).
// After migrate_plaintext_documents_to_vault, originals live as encrypted
// records in VAULT/records/*.enc.json. Their METADATA is plaintext-indexable
// without unlock; CONTENT needs an unlocked Vault. These helpers keep
// listing/search honest: nothing is fabricated when content is unavailable.
// ---------------------------------------------------------------------------

/// One plaintext-indexable record from the encrypted store.
pub(crate) struct RecordFileEntry {
    pub(crate) record_id: String,
    pub(crate) record_type: RecordType,
    pub(crate) parent_record_id: Option<String>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    /// Deleted records (their file now holds a tombstone envelope) must not
    /// be listed or searched — a deletion on any host must stick here too.
    pub(crate) tombstone: bool,
}

fn records_dir(vault_root: &std::path::Path) -> PathBuf {
    vault_root.join("VAULT").join("records")
}

pub(crate) fn scan_record_metadata(vault_root: &std::path::Path) -> Vec<RecordFileEntry> {
    let dir = records_dir(vault_root);
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e == "json").unwrap_or(false)
            && path
                .file_name()
                .map(|n| n.to_string_lossy().ends_with(".enc.json"))
                .unwrap_or(false)
        {
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Ok(envelope) = serde_json::from_str::<EncryptedRecord>(&text) {
                    let m = envelope.metadata;
                    out.push(RecordFileEntry {
                        record_id: m.record_id,
                        record_type: m.record_type,
                        parent_record_id: m.parent_record_id,
                        created_at: m.created_at,
                        updated_at: m.updated_at,
                        tombstone: m.tombstone,
                    });
                }
            }
        }
    }
    out
}

/// Migrated legacy ids and which record holds the original bytes.
/// Reads the migration metadata envelope (ContextSnapshot children) whose
/// content is encrypted — so legacy ids are pulled from the MIGRATION MARKER
/// only when the vault is unlocked; otherwise titles fall back to record ids.
/// We never invent data: when a legacy id cannot be resolved, the record id
/// is shown verbatim (truthful, if ugly).
fn legacy_ids_for_parents(
    records: &[RecordFileEntry],
    vault: Option<&Vault>,
) -> HashMap<String, (String, String)> {
    // parent record id -> (legacy_id, legacy_rel_path)
    let mut map = HashMap::new();
    let Some(vault) = vault else {
        return map;
    };
    for entry in records.iter().filter(|e| {
        !e.tombstone && e.record_type == RecordType::ContextSnapshot && e.parent_record_id.is_some()
    }) {
        if let Ok((_, bytes)) = vault.read_record(&entry.record_id) {
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                let parent = entry.parent_record_id.clone().unwrap();
                let legacy_id = value
                    .get("legacy_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let rel = value
                    .get("legacy_rel_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if !legacy_id.is_empty() {
                    map.insert(parent, (legacy_id, rel));
                }
            }
        }
    }
    map
}

/// An Android-authored record payload envelope, written by the phone's
/// `VaultRecordFactory`: `{"kind":"note",title,content,tags}` inside DOCUMENT
/// records, `{"kind":"memory",key,value,type}` inside MEMORY records,
/// `{"kind":"skill",...}` inside DOCUMENT records (skills mirror as
/// documents), `{"kind":"transcript",sessionId,role,content,inputType}`
/// inside TRANSCRIPT records (the universal conversation history) and
/// `{"kind":"envobs",subject,observedCapability,...}` inside DOCUMENT records
/// (P1-C user-confirmed capability facts). The desktop's own env-learning
/// producer writes `{"kind":"procedure_outcome",procedureId,...,outcomeJson}`
/// inside TOOL_RESULT records.
/// Read-compat only — the desktop never re-writes these records in the
/// Android format; it reads them so a shared drive shows the same notes,
/// memories, skills, conversations and confirmed capabilities on every host.
struct AndroidEnvelope {
    kind: String,
    title_or_key: String,
    body: String,
    tags: Vec<String>,
}

fn parse_android_envelope(bytes: &[u8]) -> Option<AndroidEnvelope> {
    let value = serde_json::from_slice::<serde_json::Value>(bytes).ok()?;
    let kind = value.get("kind")?.as_str()?.to_string();
    match kind.as_str() {
        "note" => Some(AndroidEnvelope {
            kind,
            title_or_key: value.get("title")?.as_str()?.to_string(),
            body: value
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            tags: value
                .get("tags")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .collect(),
        }),
        "memory" => Some(AndroidEnvelope {
            kind,
            title_or_key: value.get("key")?.as_str()?.to_string(),
            body: value
                .get("value")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            tags: Vec::new(),
        }),
        "skill" => Some(AndroidEnvelope {
            kind,
            title_or_key: value.get("name")?.as_str()?.to_string(),
            body: value
                .get("stepsJson")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            tags: vec!["skill".to_string()],
        }),
        "transcript" => {
            let session = value.get("sessionId")?.as_str()?;
            let role = value.get("role")?.as_str()?;
            Some(AndroidEnvelope {
                kind,
                title_or_key: format!("Conversation {} — {}", session, role),
                body: value
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                tags: vec!["transcript".to_string()],
            })
        }
        // P1-C: a user-confirmed capability fact authored on another host.
        // The envelope fields let this host list it without parsing the
        // contract body; only the phone's verified facts and corrections ever
        // mirror (hypotheses stay device-local and never appear here).
        "envobs" => {
            let subject = value.get("subject")?.as_str()?;
            let status = value
                .get("epistemicStatus")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let capability = value
                .get("observedCapability")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Some(AndroidEnvelope {
                kind,
                title_or_key: subject.to_string(),
                body: format!("confirmed capability: {capability} ({status})"),
                tags: vec!["env-fact".to_string()],
            })
        }
        // The desktop env-learning producer's own telemetry envelope.
        "procedure_outcome" => {
            let procedure_id = value.get("procedureId")?.as_str()?;
            let result = value
                .get("result")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            Some(AndroidEnvelope {
                kind,
                title_or_key: format!("Procedure {procedure_id} — {result}"),
                body: String::new(), // telemetry: the contract body is not user prose
                tags: vec!["procedure-outcome".to_string()],
            })
        }
        _ => None,
    }
}

/// List documents whose originals live as encrypted records (parentless
/// Document originals, plus the Android-authored skills and conversation
/// turns that mirror as parentless DOCUMENT/TRANSCRIPT records). Metadata is
/// honest even locked; size bytes are unknown without decrypt and are
/// reported as None-equivalent zero with a derived title. When the vault is
/// unlocked, an Android-authored note/skill/transcript envelope yields the
/// REAL title and tags; otherwise the title falls back to the record id
/// (truthful, if ugly) — never an invented title.
pub fn list_migrated_documents(vault_root: &str, vault: Option<&Vault>) -> Vec<DocumentMetadata> {
    let root = PathBuf::from(vault_root);
    let records = scan_record_metadata(&root);
    let legacy = legacy_ids_for_parents(&records, vault);
    records
        .iter()
        .filter(|e| {
            !e.tombstone
                && e.parent_record_id.is_none()
                && matches!(e.record_type, RecordType::Document | RecordType::Transcript)
        })
        .map(|e| {
            // Prefer the decrypted Android envelope when we can read it.
            let android = vault.and_then(|v| {
                v.read_record(&e.record_id)
                    .ok()
                    .and_then(|(_, bytes)| parse_android_envelope(&bytes))
                    .filter(|env| {
                        matches!(
                            env.kind.as_str(),
                            "note" | "skill" | "transcript" | "envobs"
                        )
                    })
            });
            let (title, _rel) = legacy
                .get(&e.record_id)
                .cloned()
                .unwrap_or_else(|| (e.record_id.clone(), String::new()));
            let title = android
                .as_ref()
                .map(|env| env.title_or_key.clone())
                .unwrap_or(title);
            DocumentMetadata {
                id: e.record_id.clone(),
                title,
                document_type: DocumentType::Txt, // type unknown without decrypt; truthfully generic
                file_path: format!("vault://records/{}", e.record_id),
                file_size_bytes: 0, // unknown without decryption — never invented
                created_at: e.created_at.clone(),
                modified_at: e.updated_at.clone(),
                source_platform: "DESKTOP".to_string(),
                tags: android
                    .map(|env| env.tags)
                    .unwrap_or_else(|| vec!["migrated".to_string()]),
                page_count: None,
                word_count: None,
            }
        })
        .collect()
}

/// The TF-IDF scorer shared by plaintext memory files and decrypted migrated
/// records. `memory_type` for migrated records is "migrated" so callers can
/// distinguish provenance (never hidden from the caller).
fn score_with_tfidf(
    query: &MemorySearchQuery,
    file_contents: Vec<(String, String, String, String, String)>,
    provenance: &str,
) -> Vec<MemorySearchResult> {
    if file_contents.is_empty() {
        return Vec::new();
    }

    // Wildcard query — return all entries with relevance 1.0
    if query.query == "*" {
        let mut results: Vec<MemorySearchResult> = file_contents
            .into_iter()
            .map(|(id, title, content, memory_type, modified_at)| {
                // Grapheme-safe: raw byte slicing panicked mid-character
                // on Devanagari/Bengali/Assamese input.
                let preview = unoone_text::preview(&content, 200);
                MemorySearchResult {
                    id: id.clone(),
                    memory_type,
                    title,
                    preview,
                    relevance: 1.0,
                    created_at: modified_at,
                }
            })
            .collect();
        results.truncate(query.limit as usize);
        return results;
    }

    let query_terms = tokenize(&query.query.to_lowercase());
    if query_terms.is_empty() {
        return Vec::new();
    }

    let num_docs = file_contents.len() as f32;
    let mut doc_freq: HashMap<String, u32> = HashMap::new();
    for (_, _, content, _, _) in &file_contents {
        let unique_terms: std::collections::HashSet<String> =
            tokenize(&content.to_lowercase()).into_iter().collect();
        for term in unique_terms {
            *doc_freq.entry(term).or_insert(0) += 1;
        }
    }

    let mut scored: Vec<MemorySearchResult> = file_contents
        .into_iter()
        .filter_map(|(id, title, content, memory_type, modified_at)| {
            if !query.memory_types.is_empty()
                && !query
                    .memory_types
                    .iter()
                    .any(|t| memory_type.starts_with(t))
            {
                return None;
            }
            let content_lower = content.to_lowercase();
            let content_terms = tokenize(&content_lower);
            if content_terms.is_empty() {
                return None;
            }
            let mut tf: HashMap<String, u32> = HashMap::new();
            for term in &content_terms {
                *tf.entry(term.clone()).or_insert(0) += 1;
            }
            let mut score = 0.0f32;
            let mut matched = 0u32;
            for qterm in &query_terms {
                if let Some(&count) = tf.get(qterm) {
                    let tf_score = (count as f32) / (content_terms.len() as f32).sqrt();
                    let df = *doc_freq.get(qterm).unwrap_or(&0);
                    let idf_score = if num_docs > 1.0 {
                        (1.0 + (num_docs - df as f32 + 0.5) / (df as f32 + 0.5)).ln()
                    } else {
                        1.0
                    };
                    score += tf_score * idf_score;
                    matched += 1;
                }
            }
            if matched == 0 || score < query.min_relevance * 0.1 {
                return None;
            }
            score = score.min(1.0);
            let preview = unoone_text::preview(&content, 200);
            Some(MemorySearchResult {
                id: id.clone(),
                memory_type: if provenance.is_empty() {
                    memory_type
                } else {
                    provenance.to_string()
                },
                title,
                preview,
                relevance: score,
                created_at: modified_at,
            })
        })
        .collect();

    scored.sort_by(|a, b| {
        b.relevance
            .partial_cmp(&a.relevance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(query.limit as usize);
    scored
}

/// TF-IDF over DECRYPTED migrated records (Memory originals + Transcript
/// children of migrated Documents). Only runs when the vault is unlocked —
/// an empty vault handle honestly yields zero extra results, never an error.
pub fn search_migrated_contents(
    query: &MemorySearchQuery,
    vault_root: &str,
    vault: Option<&Vault>,
) -> Vec<MemorySearchResult> {
    let Some(vault) = vault else {
        return Vec::new();
    };
    let root = PathBuf::from(vault_root);
    let records = scan_record_metadata(&root);
    let legacy = legacy_ids_for_parents(&records, Some(vault));
    // (id, title, content, memory_type, modified_at)
    let mut contents: Vec<(String, String, String, String, String)> = Vec::new();
    for entry in &records {
        if entry.tombstone {
            continue; // deleted on any host — stays deleted here
        }
        let is_memory = entry.record_type == RecordType::Memory && entry.parent_record_id.is_none();
        // Transcript children count only when their parent is a Document —
        // Memory originals also get a Transcript child during migration, and
        // searching both would duplicate every memory hit.
        let is_document_transcript = entry.record_type == RecordType::Transcript
            && entry.parent_record_id.as_ref().is_some_and(|p| {
                records
                    .iter()
                    .any(|par| par.record_id == *p && par.record_type == RecordType::Document)
            });
        // Android conversation turns mirror as parentless TRANSCRIPT records —
        // the universal usage history, searchable like any other memory.
        let is_android_turn =
            entry.record_type == RecordType::Transcript && entry.parent_record_id.is_none();
        // P1-C: env-learning records join the search surface only when their
        // DECRYPTED envelope is the shared learning envelope. Document
        // originals (notes, migrated docs, skills) and other tool results
        // stay out of memory search exactly as before.
        let maybe_env_fact =
            entry.record_type == RecordType::Document && entry.parent_record_id.is_none();
        let maybe_procedure_outcome =
            entry.record_type == RecordType::ToolResult && entry.parent_record_id.is_none();
        if !is_memory
            && !is_document_transcript
            && !is_android_turn
            && !maybe_env_fact
            && !maybe_procedure_outcome
        {
            continue;
        }
        let Ok((_, bytes)) = vault.read_record(&entry.record_id) else {
            continue; // a record we cannot decrypt is omitted, not broken over
        };
        if maybe_env_fact {
            let Some(env) = parse_android_envelope(&bytes).filter(|env| env.kind == "envobs")
            else {
                continue; // an ordinary document original — not memory search material
            };
            // Only a phone's user-confirmed facts/corrections ever mirror,
            // so a hit here is a capability the user approved on some host.
            contents.push((
                entry.record_id.clone(),
                env.title_or_key.clone(),
                format!("{}: {}", env.title_or_key, env.body),
                "env-fact".to_string(),
                entry.updated_at.clone(),
            ));
            continue;
        }
        if maybe_procedure_outcome {
            let Some(env) =
                parse_android_envelope(&bytes).filter(|env| env.kind == "procedure_outcome")
            else {
                continue; // an ordinary tool result — not memory search material
            };
            // Telemetry searches over the envelope text (procedure id, route,
            // result) — honest, and excluded from the agent's four-type
            // filter unless a caller asks for it.
            let Ok(raw) = String::from_utf8(bytes) else {
                continue;
            };
            contents.push((
                entry.record_id.clone(),
                env.title_or_key,
                raw,
                "procedure-outcome".to_string(),
                entry.updated_at.clone(),
            ));
            continue;
        }
        if is_memory {
            // Android-authored MEMORY records carry a {kind:"memory",key,value}
            // envelope: search "key: value" (clean tokens, real title). Raw
            // legacy memory bytes search verbatim.
            let (title, content) = match parse_android_envelope(&bytes) {
                Some(env) if env.kind == "memory" => {
                    let title = env.title_or_key;
                    let content = format!("{}: {}", title, env.body);
                    (title, content)
                }
                _ => {
                    let Ok(raw) = String::from_utf8(bytes) else {
                        continue; // binary memory content honestly excluded from text search
                    };
                    (entry.record_id.clone(), raw)
                }
            };
            contents.push((
                entry.record_id.clone(),
                title,
                content,
                "memory".to_string(),
                entry.updated_at.clone(),
            ));
        } else if is_android_turn {
            // A conversation turn authored on any host. With the shared
            // envelope it searches as "Conversation <session> — <role>" and
            // is typed "transcript"; anything else is raw text searched
            // verbatim under the same honest type.
            let (title, content) = match parse_android_envelope(&bytes) {
                Some(env) if env.kind == "transcript" => (env.title_or_key, env.body),
                _ => {
                    let Ok(raw) = String::from_utf8(bytes) else {
                        continue; // binary content honestly excluded from text search
                    };
                    (entry.record_id.clone(), raw)
                }
            };
            contents.push((
                entry.record_id.clone(),
                title,
                content,
                "transcript".to_string(),
                entry.updated_at.clone(),
            ));
        } else {
            let content = match String::from_utf8(bytes) {
                Ok(c) => c,
                Err(_) => continue, // binary content honestly excluded from text search
            };
            let id = entry
                .parent_record_id
                .as_ref()
                .and_then(|p| legacy.get(p).map(|(lid, _)| lid.clone()))
                .unwrap_or_else(|| entry.record_id.clone());
            let title = id.clone();
            contents.push((
                id,
                title,
                content,
                "document".to_string(),
                entry.updated_at.clone(),
            ));
        }
    }
    // Keep type names compatible with the agent's memory_types filter
    // (["note","document","memory","transcript"] — see agent.rs search_notes)
    // — starts_with matching applies, so the provenance must stay "" (a
    // "migrated" label here would be filtered out).
    score_with_tfidf(query, contents, "")
}

/// Read one migrated document's content. [id] is whatever `list_migrated_documents`
/// surfaced: a pre-migration legacy id (desktop-migrated documents) or a vault
/// record id (Android-authored notes list their record id, and locked listings
/// fall back to it too). Returns the ORIGINAL bytes. Only works unlocked;
/// returns None otherwise.
pub fn read_migrated_document_content(
    vault_root: &str,
    id: &str,
    vault: &Vault,
) -> Option<Vec<u8>> {
    let root = PathBuf::from(vault_root);
    let records = scan_record_metadata(&root);
    // Direct record-id match first (no decrypt needed to resolve). Android
    // conversation turns surface their record id too, so they read back the
    // same way notes do.
    if let Some(direct) = records.iter().find(|e| {
        !e.tombstone
            && matches!(e.record_type, RecordType::Document | RecordType::Transcript)
            && e.parent_record_id.is_none()
            && e.record_id == id
    }) {
        return vault.read_record(&direct.record_id).ok().map(|(_, b)| b);
    }
    // Otherwise resolve through the migration envelope's legacy id.
    let legacy = legacy_ids_for_parents(&records, Some(vault));
    // Document original whose migration envelope carries this legacy id.
    let target = records.iter().find(|e| {
        !e.tombstone
            && e.record_type == RecordType::Document
            && e.parent_record_id.is_none()
            && legacy
                .get(&e.record_id)
                .map(|(lid, _)| lid == id)
                .unwrap_or(false)
    })?;
    let (_, bytes) = vault.read_record(&target.record_id).ok()?;
    Some(bytes)
}

// Tauri commands

#[tauri::command]
pub fn list_documents(
    vault_root: String,
    vault_state: tauri::State<'_, crate::DesktopVaultState>,
) -> Vec<DocumentMetadata> {
    let processor = DocumentProcessor::new(&vault_root);
    let mut documents = processor.list_documents();
    let guard = vault_state.vault.lock().ok();
    let vault_ref = guard.as_ref().and_then(|v| v.as_ref());
    let mut migrated = list_migrated_documents(&vault_root, vault_ref);
    let seen: std::collections::HashSet<String> = documents.iter().map(|d| d.id.clone()).collect();
    migrated.retain(|d| !seen.contains(&d.id));
    documents.append(&mut migrated);
    documents
}

/// Chat attachment the frontend could not interpret itself (PDF/DOCX/XLSX/
/// PPTX), extracted server-side through the same audited parser lane the
/// document processor uses. The bytes arrive base64-encoded from the
/// WebView file picker (which deliberately does not hand out host paths),
/// are decoded under a hard size cap, parsed from a temp file that is
/// deleted afterwards, and only the extracted text ever reaches the chat.
#[derive(Debug, Clone, Serialize)]
pub struct ParsedAttachment {
    pub kind: String,
    pub text: String,
    pub truncated: bool,
}

/// Hard cap on a single attached document. Generous for real documents,
/// small enough that a hostile attachment cannot exhaust memory.
const ATTACHED_DOCUMENT_MAX_BYTES: usize = 20 * 1024 * 1024;

#[tauri::command]
pub fn parse_attached_document(
    filename: String,
    data_base64: String,
) -> Result<ParsedAttachment, String> {
    use base64::Engine as _;

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_base64.trim())
        .map_err(|e| format!("Attachment is not valid base64: {}", e))?;
    if bytes.len() > ATTACHED_DOCUMENT_MAX_BYTES {
        return Err(format!(
            "Attachment is {} MiB; the limit is {} MiB.",
            bytes.len() / (1024 * 1024),
            ATTACHED_DOCUMENT_MAX_BYTES / (1024 * 1024)
        ));
    }

    let ext = std::path::Path::new(&filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    let kind = match ext.as_str() {
        "pdf" => "pdf",
        "docx" => "docx",
        "xlsx" => "xlsx",
        "pptx" => "pptx",
        _ => "text",
    }
    .to_owned();

    // Parse from a temp file: every extractor in this module is path-based
    // (lopdf and the zip readers need seekable files). The file is deleted
    // in the same breath as parsing finishes.
    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join(format!(
        "unoone-chat-attach-{}-{}.{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
        if ext.is_empty() {
            "txt".to_owned()
        } else {
            ext.clone()
        }
    ));
    std::fs::write(&temp_path, &bytes)
        .map_err(|e| format!("Failed to stage attachment for parsing: {}", e))?;
    let text = match ext.as_str() {
        "pdf" => extract_pdf_text(&temp_path),
        "docx" => extract_docx_text(&temp_path),
        "xlsx" => extract_xlsx_text(&temp_path),
        "pptx" => extract_pptx_text(&temp_path),
        // Plain-text and code attachments: utf-8 decode (lossy for binary
        // junk) plus the shared truncation notice.
        _ => Ok(unoone_text::truncate_bytes_with_notice(
            &String::from_utf8_lossy(&bytes),
            8000,
        )),
    };
    let _ = std::fs::remove_file(&temp_path);

    let text = text?;
    // truncate_bytes_with_notice marks a cut with a trailing notice line;
    // surfacing that as a flag lets the chat UI label the attachment.
    const TRUNCATION_NOTICE: &str = "[Truncated — ";
    Ok(ParsedAttachment {
        kind,
        truncated: text.contains(TRUNCATION_NOTICE),
        text,
    })
}

#[tauri::command]
pub fn search_memories(
    query: MemorySearchQuery,
    vault_root: String,
    vault_state: tauri::State<'_, crate::DesktopVaultState>,
) -> Vec<MemorySearchResult> {
    let processor = DocumentProcessor::new(&vault_root);
    let guard = vault_state.vault.lock().ok();
    let vault_ref = guard.as_ref().and_then(|v| v.as_ref());
    let mut results = processor.search_memories(&query);
    let mut migrated = search_migrated_contents(&query, &vault_root, vault_ref);
    results.append(&mut migrated);
    results
}

#[cfg(test)]
mod migrated_readpath_tests {
    use super::*;
    use crate::document_migration;

    fn fixture() -> (tempfile::TempDir, Vault, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let vault_root = dir.path().join("UNOONE");
        let _ = Vault::create(&vault_root, b"synthetic-readpath-test-pw").unwrap();
        let mut vault = Vault::open(&vault_root).unwrap();
        vault.unlock(b"synthetic-readpath-test-pw").unwrap();
        let docs = vault_root.join("VAULT").join("documents");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(
            docs.join("synth-notes.txt"),
            "synthetic turmeric latte recipe notes",
        )
        .unwrap();
        let mem = vault_root.join("VAULT").join("memory");
        std::fs::create_dir_all(&mem).unwrap();
        std::fs::write(
            mem.join("quick-list.md"),
            "synthetic memory: call the cardamom supplier",
        )
        .unwrap();
        document_migration::migrate(&mut vault, vault_root.as_path()).unwrap();
        (dir, vault, vault_root)
    }

    #[test]
    fn migrated_docs_are_listed_and_found_and_readable() {
        let (_dir, vault, root) = fixture();
        let root_s = root.to_string_lossy().to_string();

        let listed = list_migrated_documents(&root_s, Some(&vault));
        assert_eq!(
            listed.len(),
            1,
            "only the document original (memories are not Documents)"
        );
        assert_eq!(
            listed[0].title, "synth-notes",
            "legacy title resolved from migration envelope"
        );
        assert_eq!(listed[0].tags, vec!["migrated".to_string()]);

        // Search over decrypted migrated contents finds the migrated memory text.
        let query = MemorySearchQuery {
            query: "cardamom".to_string(),
            memory_types: vec![],
            limit: 10,
            min_relevance: 0.0,
        };
        let found = search_migrated_contents(&query, &root_s, Some(&vault));
        assert_eq!(
            found.len(),
            1,
            "expected exactly one migrated hit: {found:?}"
        );

        // Single-document read returns the original bytes.
        let bytes =
            read_migrated_document_content(&root_s, "synth-notes", &vault).expect("content");
        assert!(String::from_utf8(bytes).unwrap().contains("turmeric"));
    }

    #[test]
    fn locked_vault_yields_empty_results_not_errors() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_string_lossy().to_string();
        let query = MemorySearchQuery {
            query: "anything".to_string(),
            memory_types: vec![],
            limit: 10,
            min_relevance: 0.0,
        };
        // Unlocked-handle absence: all three paths return empty/None.
        assert!(search_migrated_contents(&query, &root, None).is_empty());
        assert!(list_migrated_documents(&root, None).is_empty());
    }

    // ---- Android-authored record interop (read-compat) ----------------------
    // The phone's VaultRecordFactory writes {kind:"note"...} into DOCUMENT
    // records and {kind:"memory"...} into MEMORY records. The desktop must
    // read them back with real titles/keys, not raw record ids.

    fn android_vault(dir_path: &std::path::Path) -> (Vault, String) {
        let vault_root = dir_path.join("UNOONE");
        let _ = Vault::create(&vault_root, b"synthetic-android-interop-pw").unwrap();
        let mut vault = Vault::open(&vault_root).unwrap();
        vault.unlock(b"synthetic-android-interop-pw").unwrap();
        (vault, vault_root.to_string_lossy().to_string())
    }

    fn write_android_record(vault: &mut Vault, record_type: RecordType, payload: &str) -> String {
        let mut record = unoone_vault_core::Record::new(record_type, "ANDROID", "phone-uuid");
        record.origin_platform = "ANDROID".to_string();
        let id = record.record_id.clone();
        vault.write_record(record, payload.as_bytes()).unwrap();
        id
    }

    #[test]
    fn android_env_fact_lists_and_searches_as_a_confirmed_capability() {
        let dir = tempfile::tempdir().unwrap();
        let (mut vault, root) = android_vault(dir.path());
        // The exact envelope VaultRecordFactory.forEnvFact writes (a
        // user-confirmed capability approved on the phone).
        let observation = serde_json::json!({
            "schema": "inbharat.pai.envobs.v1",
            "subject": "skill:Suggested · Open Calendar",
            "observed_capability": "execute skill 'Suggested · Open Calendar'",
            "evidence": "user explicitly enabled skill 'Suggested · Open Calendar'",
            "confidence": "high",
            "scope": "user",
            "epistemic_status": "verified_fact",
            "provenance": {"platform": "android", "device_id": "phone-uuid", "source": "user-approval"},
            "timestamp_ms": 1_760_000_000_000u64,
            "verification_ref": "user_enabled_skill:Suggested · Open Calendar@1760000000000",
        });
        let payload = serde_json::json!({
            "kind": "envobs",
            "subject": "skill:Suggested · Open Calendar",
            "observedCapability": "execute skill 'Suggested · Open Calendar'",
            "epistemicStatus": "verified_fact",
            "verificationRef": "user_enabled_skill:Suggested · Open Calendar@1760000000000",
            "observationJson": observation.to_string(),
        })
        .to_string();
        let fact_id = write_android_record(&mut vault, RecordType::Document, &payload);

        // Lists as a document with the subject as its honest title.
        let listed = list_migrated_documents(&root, Some(&vault));
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, "skill:Suggested · Open Calendar");
        assert_eq!(listed[0].tags, vec!["env-fact".to_string()]);
        assert_eq!(listed[0].id, fact_id);
        // Reads back by record id (the raw envelope bytes).
        let bytes = read_migrated_document_content(&root, &fact_id, &vault)
            .expect("env fact must be readable");
        assert!(String::from_utf8(bytes).unwrap().contains("envobs"));

        // Searches under its own honest type.
        let query = MemorySearchQuery {
            query: "calendar".to_string(),
            memory_types: vec![],
            limit: 10,
            min_relevance: 0.0,
        };
        let found = search_migrated_contents(&query, &root, Some(&vault));
        assert_eq!(found.len(), 1, "expected exactly one hit: {found:?}");
        assert_eq!(found[0].memory_type, "env-fact");
        assert!(found[0].preview.contains("confirmed capability"));
    }

    #[test]
    fn desktop_procedure_outcome_telemetry_searches_under_its_own_type() {
        let dir = tempfile::tempdir().unwrap();
        let (mut vault, root) = android_vault(dir.path());
        // The envelope crate::env_learning writes for one completed harness run.
        let outcome = serde_json::json!({
            "schema": "inbharat.pai.procedure.v1",
            "procedure_id": "harness.agent_run",
            "result": "success",
            "risk_class": "DIRECT",
            "promotion": {"status": "none", "policy_version": "harness-run-policy-v1"},
        });
        let payload = serde_json::json!({
            "kind": "procedure_outcome",
            "procedureId": "harness.agent_run",
            "result": "success",
            "riskClass": "DIRECT",
            "status": "none",
            "outcomeJson": outcome,
        })
        .to_string();
        write_android_record(&mut vault, RecordType::ToolResult, &payload);

        let unfiltered = MemorySearchQuery {
            query: "harness".to_string(),
            memory_types: vec![],
            limit: 10,
            min_relevance: 0.0,
        };
        let found = search_migrated_contents(&unfiltered, &root, Some(&vault));
        assert_eq!(found.len(), 1, "expected exactly one hit: {found:?}");
        assert_eq!(found[0].memory_type, "procedure-outcome");
        assert!(found[0].title.contains("harness.agent_run"));

        // Telemetry never leaks into the agent's four-type memory filter.
        let agent_typed = MemorySearchQuery {
            query: "harness".to_string(),
            memory_types: vec![
                "note".to_string(),
                "document".to_string(),
                "memory".to_string(),
                "transcript".to_string(),
            ],
            limit: 10,
            min_relevance: 0.0,
        };
        assert!(search_migrated_contents(&agent_typed, &root, Some(&vault)).is_empty());
    }

    #[test]
    fn android_note_lists_with_real_title_and_reads_by_record_id() {
        let dir = tempfile::tempdir().unwrap();
        let (mut vault, root) = android_vault(dir.path());
        let payload = r#"{"kind":"note","title":"Groceries","content":"turmeric and cardamom","tags":"list,errand"}"#;
        let note_id = write_android_record(&mut vault, RecordType::Document, payload);

        // Unlocked: the envelope yields the real title and tags.
        let listed = list_migrated_documents(&root, Some(&vault));
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0].title, "Groceries",
            "Android note must show its real title, not the record id"
        );
        assert_eq!(
            listed[0].tags,
            vec!["list".to_string(), "errand".to_string()]
        );
        assert_eq!(listed[0].id, note_id);

        // The id surfaced by the listing (the record id) must read back.
        let bytes = read_migrated_document_content(&root, &note_id, &vault)
            .expect("Android note must be readable by its record id");
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("turmeric"));
    }

    #[test]
    fn locked_listing_of_android_note_falls_back_to_record_id() {
        let dir = tempfile::tempdir().unwrap();
        let (mut vault, root) = android_vault(dir.path());
        let payload = r#"{"kind":"note","title":"Groceries","content":"turmeric","tags":""}"#;
        let note_id = write_android_record(&mut vault, RecordType::Document, payload);
        drop(vault);

        // Locked: no decrypt, so the title is the record id — honest, never invented.
        let listed = list_migrated_documents(&root, None);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, note_id);
    }

    #[test]
    fn android_memory_searches_by_value_with_key_as_title() {
        let dir = tempfile::tempdir().unwrap();
        let (mut vault, root) = android_vault(dir.path());
        let payload =
            r#"{"kind":"memory","key":"wake_word","value":"namaste deva","type":"preference"}"#;
        write_android_record(&mut vault, RecordType::Memory, payload);

        let query = MemorySearchQuery {
            query: "namaste".to_string(),
            memory_types: vec![],
            limit: 10,
            min_relevance: 0.0,
        };
        let found = search_migrated_contents(&query, &root, Some(&vault));
        assert_eq!(found.len(), 1, "expected exactly one hit: {found:?}");
        assert_eq!(
            found[0].title, "wake_word",
            "memory title must be the envelope key, not the record id"
        );
        assert!(found[0].preview.contains("namaste deva"));

        // The agent's memory_types filter must still match ("memory").
        let typed = MemorySearchQuery {
            query: "namaste".to_string(),
            memory_types: vec!["memory".to_string()],
            limit: 10,
            min_relevance: 0.0,
        };
        assert_eq!(
            search_migrated_contents(&typed, &root, Some(&vault)).len(),
            1
        );
    }

    #[test]
    fn tombstoned_records_are_never_listed_or_searched() {
        let dir = tempfile::tempdir().unwrap();
        let (mut vault, root) = android_vault(dir.path());
        let note_payload = r#"{"kind":"note","title":"Secret","content":"deleted soon","tags":""}"#;
        let note_id = write_android_record(&mut vault, RecordType::Document, note_payload);
        let mem_payload =
            r#"{"kind":"memory","key":"old_pref","value":"cardamom forever","type":"preference"}"#;
        let mem_id = write_android_record(&mut vault, RecordType::Memory, mem_payload);

        // Delete both on the phone (tombstone) — the desktop must respect it.
        vault
            .delete_record(&note_id, "ANDROID", "phone-uuid")
            .unwrap();
        vault
            .delete_record(&mem_id, "ANDROID", "phone-uuid")
            .unwrap();

        assert!(
            list_migrated_documents(&root, Some(&vault)).is_empty(),
            "a deleted note must not come back on the desktop"
        );
        let query = MemorySearchQuery {
            query: "cardamom".to_string(),
            memory_types: vec![],
            limit: 10,
            min_relevance: 0.0,
        };
        assert!(
            search_migrated_contents(&query, &root, Some(&vault)).is_empty(),
            "a deleted memory must not come back on the desktop"
        );
        assert!(read_migrated_document_content(&root, &note_id, &vault).is_none());
    }

    #[test]
    fn android_transcript_turn_lists_searches_and_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let (mut vault, root) = android_vault(dir.path());
        let payload = r#"{"kind":"transcript","sessionId":"sess-42","role":"user","content":"call ravi please","inputType":"voice"}"#;
        let turn_id = write_android_record(&mut vault, RecordType::Transcript, payload);

        // Listing: real conversation title, transcript tag.
        let listed = list_migrated_documents(&root, Some(&vault));
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, "Conversation sess-42 — user");
        assert_eq!(listed[0].tags, vec!["transcript".to_string()]);

        // Search: the conversation is retrievable and typed "transcript".
        let query = MemorySearchQuery {
            query: "ravi".to_string(),
            memory_types: vec![],
            limit: 10,
            min_relevance: 0.0,
        };
        let found = search_migrated_contents(&query, &root, Some(&vault));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].memory_type, "transcript");
        assert!(found[0].preview.contains("call ravi please"));

        // The agent's memory_types filter matches "transcript" (agent.rs).
        let typed = MemorySearchQuery {
            query: "ravi".to_string(),
            memory_types: vec!["transcript".to_string()],
            limit: 10,
            min_relevance: 0.0,
        };
        assert_eq!(
            search_migrated_contents(&typed, &root, Some(&vault)).len(),
            1
        );

        // Read back through the same id the listing surfaced.
        let bytes = read_migrated_document_content(&root, &turn_id, &vault)
            .expect("Android conversation turn must be readable by its record id");
        assert!(String::from_utf8(bytes)
            .unwrap()
            .contains("call ravi please"));
    }

    #[test]
    fn locked_listing_of_android_turn_falls_back_to_record_id() {
        let dir = tempfile::tempdir().unwrap();
        let (mut vault, root) = android_vault(dir.path());
        let payload = r#"{"kind":"transcript","sessionId":"s1","role":"assistant","content":"Done.","inputType":"voice"}"#;
        let turn_id = write_android_record(&mut vault, RecordType::Transcript, payload);
        drop(vault);

        // Locked: no decrypt, so the title is the record id — honest, never invented.
        let listed = list_migrated_documents(&root, None);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, turn_id);
    }

    #[test]
    fn android_skill_lists_with_its_real_name() {
        let dir = tempfile::tempdir().unwrap();
        let (mut vault, root) = android_vault(dir.path());
        let payload = r#"{"kind":"skill","name":"morning briefing","triggerPhrases":"brief me","stepsJson":"[\"read_screen\"]","riskLevel":0,"enabled":true}"#;
        write_android_record(&mut vault, RecordType::Document, payload);

        let listed = list_migrated_documents(&root, Some(&vault));
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0].title, "morning briefing",
            "a mirrored skill must show its real name, not the record id"
        );
        assert_eq!(listed[0].tags, vec!["skill".to_string()]);
    }
}

#[cfg(test)]
mod parse_attached_document_tests {
    use super::*;

    fn b64(data: &[u8]) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(data)
    }

    #[test]
    fn text_attachment_round_trips() {
        let parsed = parse_attached_document(
            "notes.md".to_owned(),
            b64(b"# Title\nBody text for the chat attachment."),
        )
        .expect("parse");
        assert_eq!(parsed.kind, "text");
        assert!(!parsed.truncated);
        assert!(parsed.text.contains("Body text for the chat attachment."));
    }

    #[test]
    fn long_text_attachment_is_truncated_with_a_notice() {
        let long = "word ".repeat(10_000);
        let parsed =
            parse_attached_document("big.txt".to_owned(), b64(long.as_bytes())).expect("parse");
        assert!(parsed.truncated);
        assert!(parsed.text.contains("[Truncated — "));
    }

    #[test]
    fn oversized_attachment_is_rejected_before_parsing() {
        let mut data = vec![0u8; ATTACHED_DOCUMENT_MAX_BYTES + 1];
        data[0] = b'x';
        let err = parse_attached_document("huge.bin".to_owned(), b64(&data)).unwrap_err();
        assert!(err.contains("limit"), "unexpected error: {err}");
    }

    #[test]
    fn invalid_base64_is_rejected() {
        let err =
            parse_attached_document("x.txt".to_owned(), "!!!not-base64!!!".to_owned()).unwrap_err();
        assert!(err.contains("base64"), "unexpected error: {err}");
    }

    #[test]
    fn image_bytes_reported_as_text_do_not_panic() {
        // The chat frontend routes images through the vision lane; if
        // something posts raw PNG bytes here anyway, the lossy utf-8 decode
        // path must return text (garbled is fine), not an error or panic.
        let png = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0xff];
        let parsed = parse_attached_document("photo.png".to_owned(), b64(&png)).expect("parse");
        assert_eq!(parsed.kind, "text");
    }

    #[test]
    fn pdf_attachment_extracts_real_text() {
        // Hand-built minimal PDF (tests/fixtures/minimal-report.pdf) with
        // one `(sentence) Tj` per page — the exact shape the lopdf-based
        // extractor parses — so the PDF attachment lane has a regression
        // test that does not depend on an external PDF library.
        let pdf = include_bytes!("../tests/fixtures/minimal-report.pdf");
        let parsed =
            parse_attached_document("minimal-report.pdf".to_owned(), b64(pdf)).expect("parse");
        assert_eq!(parsed.kind, "pdf");
        assert!(!parsed.truncated);
        assert!(
            parsed.text.contains("Meridian Foods"),
            "extraction missed the company name: {:?}",
            parsed.text
        );
        assert!(
            parsed.text.contains("Howrah"),
            "extraction missed the second page: {:?}",
            parsed.text
        );
    }
}
