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
                                                    "[PDF extracted, {} words]",
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
        let mut file_contents: Vec<(String, String, String, String)> = Vec::new(); // (id, content, extension, modified_at)
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
                        file_contents.push((file_stem, content, ext, modified));
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

/// Extract text from a PDF using the lopdf crate for proper content stream parsing
pub(crate) fn extract_pdf_text(path: &PathBuf) -> Result<String, String> {
    let pdf = lopdf::Document::load(path).map_err(|e| format!("Failed to parse PDF: {}", e))?;

    let mut text_parts: Vec<String> = Vec::new();

    // Iterate over all pages using the page tree
    let pages = pdf.get_pages();

    for (_, page_id) in pages {
        // Get the page content stream
        match pdf.get_page_content(page_id) {
            Ok(content) => {
                // Parse the content stream for text operators
                let content_str = String::from_utf8_lossy(&content);

                // Extract text from Tj and TJ operators
                for line in content_str.lines() {
                    if let Some(text) = extract_pdf_string(line) {
                        if !text.trim().is_empty() {
                            text_parts.push(text);
                        }
                    }
                }
            }
            Err(_) => continue, // Skip pages that can't be read
        }
    }

    // If lopdf extraction yielded nothing, the PDF might be image-based
    if text_parts.is_empty() {
        return Err(
            "No text could be extracted from this PDF. It may be image-based or encrypted."
                .to_string(),
        );
    }

    let result = text_parts.join(" ");
    let word_count = result.split_whitespace().count();
    if word_count < 3 {
        return Err("PDF appears to contain no readable text. It may be image-based.".to_string());
    }

    Ok(unoone_text::truncate_bytes_with_notice(&result, 8000))
}

/// Extract a text string from a PDF Tj/TJ operator line
fn extract_pdf_string(line: &str) -> Option<String> {
    // Look for (text) Tj pattern
    if let Some(start) = line.find('(') {
        if let Some(end) = line.rfind(')') {
            if end > start {
                return Some(line[start + 1..end].to_string());
            }
        }
    }
    None
}

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
struct RecordFileEntry {
    record_id: String,
    record_type: RecordType,
    parent_record_id: Option<String>,
    created_at: String,
    updated_at: String,
}

fn records_dir(vault_root: &std::path::Path) -> PathBuf {
    vault_root.join("VAULT").join("records")
}

fn scan_record_metadata(vault_root: &std::path::Path) -> Vec<RecordFileEntry> {
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
    for entry in records
        .iter()
        .filter(|e| e.record_type == RecordType::ContextSnapshot && e.parent_record_id.is_some())
    {
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

/// List documents whose originals live as encrypted records (parentless
/// Document originals). Metadata is honest even locked; size bytes are
/// unknown without decrypt and are reported as None-equivalent zero with a
/// derived title.
pub fn list_migrated_documents(vault_root: &str, vault: Option<&Vault>) -> Vec<DocumentMetadata> {
    let root = PathBuf::from(vault_root);
    let records = scan_record_metadata(&root);
    let legacy = legacy_ids_for_parents(&records, vault);
    records
        .iter()
        .filter(|e| e.record_type == RecordType::Document && e.parent_record_id.is_none())
        .map(|e| {
            let (title, _rel) = legacy
                .get(&e.record_id)
                .cloned()
                .unwrap_or_else(|| (e.record_id.clone(), String::new()));
            DocumentMetadata {
                id: e.record_id.clone(),
                title,
                document_type: DocumentType::Txt, // type unknown without decrypt; truthfully generic
                file_path: format!("vault://records/{}", e.record_id),
                file_size_bytes: 0, // unknown without decryption — never invented
                created_at: e.created_at.clone(),
                modified_at: e.updated_at.clone(),
                source_platform: "DESKTOP".to_string(),
                tags: vec!["migrated".to_string()],
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
    file_contents: Vec<(String, String, String, String)>,
    provenance: &str,
) -> Vec<MemorySearchResult> {
    if file_contents.is_empty() {
        return Vec::new();
    }

    // Wildcard query — return all entries with relevance 1.0
    if query.query == "*" {
        let mut results: Vec<MemorySearchResult> = file_contents
            .into_iter()
            .map(|(id, content, memory_type, modified_at)| {
                // Grapheme-safe: raw byte slicing panicked mid-character
                // on Devanagari/Bengali/Assamese input.
                let preview = unoone_text::preview(&content, 200);
                MemorySearchResult {
                    id: id.clone(),
                    memory_type,
                    title: id,
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
    for (_, content, _, _) in &file_contents {
        let unique_terms: std::collections::HashSet<String> =
            tokenize(&content.to_lowercase()).into_iter().collect();
        for term in unique_terms {
            *doc_freq.entry(term).or_insert(0) += 1;
        }
    }

    let mut scored: Vec<MemorySearchResult> = file_contents
        .into_iter()
        .filter_map(|(id, content, memory_type, modified_at)| {
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
                title: id,
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
    let mut contents: Vec<(String, String, String, String)> = Vec::new();
    for entry in &records {
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
        if !is_memory && !is_document_transcript {
            continue;
        }
        let Ok((_, bytes)) = vault.read_record(&entry.record_id) else {
            continue; // a record we cannot decrypt is omitted, not broken over
        };
        let content = match String::from_utf8(bytes) {
            Ok(c) => c,
            Err(_) => continue, // binary content honestly excluded from text search
        };
        let id = if is_memory {
            entry.record_id.clone()
        } else {
            entry
                .parent_record_id
                .as_ref()
                .and_then(|p| legacy.get(p).map(|(lid, _)| lid.clone()))
                .unwrap_or_else(|| entry.record_id.clone())
        };
        // Keep type names compatible with the agent's memory_types filter
        // (["note","document","memory"]) — starts_with matching applies.
        let ext = if is_memory {
            "memory".to_string()
        } else {
            "document".to_string()
        };
        contents.push((id, content, ext, entry.updated_at.clone()));
    }
    score_with_tfidf(query, contents, "")
}

/// Read one migrated document's content by its pre-migration legacy id.
/// Returns the ORIGINAL bytes (or the extracted transcript when asked to
/// present text). Only works unlocked; returns None otherwise.
pub fn read_migrated_document_content(
    vault_root: &str,
    legacy_id: &str,
    vault: &Vault,
) -> Option<Vec<u8>> {
    let root = PathBuf::from(vault_root);
    let records = scan_record_metadata(&root);
    let legacy = legacy_ids_for_parents(&records, Some(vault));
    // Document original whose migration envelope carries this legacy id.
    let target = records.iter().find(|e| {
        e.record_type == RecordType::Document
            && e.parent_record_id.is_none()
            && legacy
                .get(&e.record_id)
                .map(|(lid, _)| lid == legacy_id)
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

#[tauri::command]
pub fn process_document(document_id: String, vault_root: String) -> DocumentProcessResult {
    let processor = DocumentProcessor::new(&vault_root);
    processor.process_document(&document_id)
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
}
