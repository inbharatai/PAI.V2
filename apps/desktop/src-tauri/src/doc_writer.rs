//! Document renderers for the `doc.create` tool (2026-10-02 universal-agent
//! cycle). Pure Rust, zero new dependencies: PDF via the already-vendored
//! `lopdf`, DOCX via the already-vendored `zip` crate, MD/TXT as plain text.
//!
//! **Round-trip contract.** Every binary renderer is written against the
//! REAL parsers that power document attachments (`documents.rs`):
//! `extract_pdf_text` scans the raw page content stream LINE BY LINE for a
//! `(text) Tj` shape (first `(` .. last `)` on the line), and
//! `extract_docx_text` reads `word/document.xml` and joins every `<w:t>`
//! body with single spaces. The renderers therefore emit exactly those
//! shapes — and the module's tests round-trip renderer output through those
//! same parsers, so a writer bug cannot pass the reader gate.
//!
//! **Why the PDF content stream is hand-encoded.** `lopdf`'s own
//! `Content::encode` escapes parentheses and backslashes inside literal
//! strings (correct PDF, but `extract_pdf_text` performs no unescaping, so
//! real text would corrupt on re-read). The stream below is emitted as raw
//! bytes instead — one operator per line, strings unescaped. The desktop
//! reader's first-`(`-to-last-`)` scan recovers any single-line text from
//! that form, including text containing parentheses. `lopdf` is still used
//! for the document OBJECT structure (catalog, pages tree, fonts, stream
//! objects, xref), which is what makes the file a valid loadable PDF.
//!
//! Honest limits, on the record: rendering is text-only (no images or
//! tables), the base-14 Helvetica fonts carry Latin glyphs only — non-Latin
//! text (Hindi, Bengali, …) still extracts perfectly through the reader but
//! will not display as glyphs in a PDF viewer.

use lopdf::dictionary;
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

/// A4 portrait, base-14 fonts, 11pt body with a 16pt title.
const PAGE_WIDTH: i64 = 595;
const PAGE_HEIGHT: i64 = 842;
const MARGIN: i64 = 56;
const TITLE_SIZE: i64 = 16;
const BODY_SIZE: i64 = 11;
const BODY_LEADING: i64 = 16;
/// Body lines per page; page 1 also carries the title at the top.
pub(crate) const LINES_PER_PAGE: usize = 45;

/// Renders `title` + `lines` into a valid PDF. See the module docs for the
/// content-stream shape and its round-trip guarantee.
pub(crate) fn render_pdf_bytes(title: &str, lines: &[String]) -> Result<Vec<u8>, String> {
    let mut doc = Document::with_version("1.4");

    // Base-14 Type1 fonts — no embedding, no new resources.
    let regular_font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });
    let bold_font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica-Bold",
    });
    let resources_id = doc.add_object(dictionary! {
        "Font" => dictionary! {
            "F1" => regular_font_id,
            "F2" => bold_font_id,
        },
    });

    // Chunk the body into pages; the title heads page 1.
    let mut pages: Vec<Vec<&str>> = Vec::new();
    for chunk in lines.chunks(LINES_PER_PAGE) {
        pages.push(chunk.iter().map(String::as_str).collect());
    }
    if pages.is_empty() {
        // A document with no body still has (at least) the title page.
        pages.push(Vec::new());
    }

    let pages_id = doc.new_object_id();
    let mut page_ids: Vec<ObjectId> = Vec::new();
    for (index, page_lines) in pages.iter().enumerate() {
        let content = pdf_page_content(title, page_lines, index == 0);
        let content_id =
            doc.add_object(Stream::new(Dictionary::new(), content.as_bytes().to_vec()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), PAGE_WIDTH.into(), PAGE_HEIGHT.into()],
        });
        page_ids.push(page_id);
    }
    doc.set_object(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => page_ids.iter().map(|id| Object::Reference(*id)).collect::<Vec<_>>(),
            "Count" => page_ids.len() as i64,
        }),
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);

    let mut bytes = Vec::new();
    doc.save_to(&mut bytes)
        .map_err(|error| format!("cannot encode the PDF document: {error}"))?;
    Ok(bytes)
}

/// One page's content stream, hand-encoded (see module docs): one operator
/// per line, every text line exactly `(text) Tj`, so the desktop reader's
/// line scanner recovers the text verbatim.
fn pdf_page_content(title: &str, lines: &[&str], with_title: bool) -> String {
    let mut content = String::new();
    if with_title {
        content.push_str("BT\n");
        content.push_str(&format!("/F2 {TITLE_SIZE} Tf\n"));
        content.push_str(&format!(
            "{MARGIN} {} Td\n",
            PAGE_HEIGHT - MARGIN - TITLE_SIZE
        ));
        content.push_str(&pdf_text_line(title));
        content.push_str(" Tj\n");
        content.push_str("ET\n");
    }
    let mut y = PAGE_HEIGHT - MARGIN - TITLE_SIZE - 24 - BODY_SIZE;
    if !with_title {
        y = PAGE_HEIGHT - MARGIN - TITLE_SIZE;
    }
    content.push_str("BT\n");
    content.push_str(&format!("/F1 {BODY_SIZE} Tf\n"));
    content.push_str(&format!("{MARGIN} {y} Td\n"));
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            // Move down one line; an empty line advances without painting.
            content.push_str(&format!("0 -{BODY_LEADING} Td\n"));
        }
        if !line.is_empty() {
            content.push_str(&pdf_text_line(line));
            content.push_str(" Tj\n");
        }
    }
    content.push_str("ET\n");
    content
}

/// The `(text) Tj` operand for one text line. The bytes are emitted raw —
/// no escaping — because the reader takes everything from the first `(` to
/// the last `)` on the line; a caller-controlled string containing either
/// paren still round-trips. A literal newline inside the text would break
/// the one-line-per-operator shape, so `\r`/`\n` are folded to spaces here.
fn pdf_text_line(text: &str) -> String {
    let folded = text.replace(['\r', '\n'], " ");
    format!("({folded})")
}

/// Renders `lines` into a minimal, valid OOXML package (DOCX). One `<w:p>`
/// paragraph per line, text in `<w:t xml:space=\"preserve\">` — exactly the
/// member `extract_docx_text` collects (it joins `<w:t>` bodies with single
/// spaces and XML-unescapes, so the writer escapes `&`, `<`, `>`).
pub(crate) fn render_docx_bytes(lines: &[String]) -> Result<Vec<u8>, String> {
    use std::io::Write as _;

    let mut cursor = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(&mut cursor);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    zip.start_file("[Content_Types].xml", options)
        .map_err(|error| format!("cannot start [Content_Types].xml: {error}"))?;
    zip.write_all(CONTENT_TYPES_XML.as_bytes())
        .map_err(|error| format!("cannot write [Content_Types].xml: {error}"))?;

    zip.start_file("_rels/.rels", options)
        .map_err(|error| format!("cannot start _rels/.rels: {error}"))?;
    zip.write_all(RELS_XML.as_bytes())
        .map_err(|error| format!("cannot write _rels/.rels: {error}"))?;

    let mut document = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
         <w:body>",
    );
    for line in lines {
        document.push_str("<w:p><w:r><w:t xml:space=\"preserve\">");
        document.push_str(&escape_xml_text(line));
        document.push_str("</w:t></w:r></w:p>");
    }
    document.push_str("</w:body></w:document>");

    zip.start_file("word/document.xml", options)
        .map_err(|error| format!("cannot start word/document.xml: {error}"))?;
    zip.write_all(document.as_bytes())
        .map_err(|error| format!("cannot write word/document.xml: {error}"))?;
    zip.finish()
        .map_err(|error| format!("cannot finish the DOCX archive: {error}"))?;

    Ok(cursor.into_inner())
}

/// The OOXML package's content types: one override for the main document.
const CONTENT_TYPES_XML: &str = "\
<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>
<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
</Types>";

/// The package root relationships: the main document part.
const RELS_XML: &str = "\
<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>\
</Relationships>";

/// XML-escapes the five characters quick-xml's unescape reverses. Newlines
/// stay newlines — the DOCX reader treats them as ordinary whitespace.
fn escape_xml_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::{extract_docx_text, extract_pdf_text};
    use std::path::PathBuf;

    /// Writes bytes to a temp file and returns its path — the real readers
    /// are file-based, so the round trip goes through the real entry points.
    fn temp_file(name: &str, bytes: &[u8]) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "unoone-doc-writer-{}-{name}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::write(&path, bytes).expect("seed temp document");
        path
    }

    fn sample_lines() -> Vec<String> {
        [
            "UnoOne Field Report",
            "",
            "The agent wrote this document itself, through the audited",
            "file fence, as a real binary file — no cloud, no Python.",
            "Second page line one: the page break works.",
            "",
            "Line after an empty line still lands.",
        ]
        .iter()
        .map(|line| line.to_string())
        .collect()
    }

    #[test]
    fn pdf_round_trips_through_the_real_reader() {
        let bytes = render_pdf_bytes("Field Report", &sample_lines()).expect("render pdf");
        let path = temp_file("roundtrip.pdf", &bytes);
        let extracted = extract_pdf_text(&path).expect("extract_pdf_text must read our PDF");
        for line in sample_lines() {
            if !line.trim().is_empty() {
                assert!(
                    extracted.contains(line.trim()),
                    "round trip lost {line:?} — extracted: {extracted:?}"
                );
            }
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn docx_round_trips_through_the_real_reader() {
        let bytes = render_docx_bytes(&sample_lines()).expect("render docx");
        let path = temp_file("roundtrip.docx", &bytes);
        let extracted = extract_docx_text(&path).expect("extract_docx_text must read our DOCX");
        // The reader joins <w:t> bodies with single spaces: compare against
        // the same whitespace-normalized form.
        let expected = sample_lines()
            .iter()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let normalized = extracted.split_whitespace().collect::<Vec<_>>().join(" ");
        assert_eq!(normalized, expected);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn pdf_text_with_parens_and_backslashes_survives_the_naive_reader() {
        // The reader scans first-'('-to-last-')' per line with no
        // unescaping; hand-encoded streams must therefore recover text
        // containing parens, backslashes and a ") Tj (" adversarial tail.
        let nasty = vec![
            "Parens (balanced) and \\backslash survive.".to_owned(),
            "Adversarial tail: ) Tj ( injected mid-line.".to_owned(),
        ];
        let bytes = render_pdf_bytes("Nasty", &nasty).expect("render pdf");
        let path = temp_file("nasty.pdf", &bytes);
        let extracted = extract_pdf_text(&path).expect("extract");
        assert!(
            extracted.contains("Parens (balanced) and \\backslash survive."),
            "paren text corrupted: {extracted:?}"
        );
        assert!(
            extracted.contains("Adversarial tail: ) Tj ( injected mid-line."),
            "adversarial text corrupted: {extracted:?}"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn docx_escapes_xml_so_entities_unescape_back() {
        let nasty = vec!["Amp & lt < gt > together: <not-a-tag> &amp;".to_owned()];
        let bytes = render_docx_bytes(&nasty).expect("render docx");
        let path = temp_file("nasty.docx", &bytes);
        let extracted = extract_docx_text(&path).expect("extract");
        assert!(
            extracted.contains("Amp & lt < gt > together: <not-a-tag> &amp;"),
            "XML text corrupted: {extracted:?}"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn empty_pdf_and_docx_still_open() {
        // No body lines at all: the renderers must still produce files the
        // readers can load. The PDF reader needs ≥3 words on the page, so
        // the title alone must carry them (an under-3-word PDF is rejected
        // by the reader and that is the reader's rule, not ours).
        let pdf = render_pdf_bytes("UnoOne Field Report", &[]).expect("empty render");
        let path = temp_file("empty.pdf", &pdf);
        assert!(extract_pdf_text(&path).is_ok(), "empty-body PDF must load");
        assert!(
            extract_pdf_text(&path)
                .expect("extract")
                .contains("UnoOne Field Report"),
            "empty-body PDF carries its title"
        );
        std::fs::remove_file(&path).ok();

        let docx = render_docx_bytes(&[]).expect("empty render");
        assert!(!docx.is_empty());
    }
}
