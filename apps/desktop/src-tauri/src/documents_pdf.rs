//! Bounded text-layer PDF extraction. No renderer, OCR, password handling or persistence.
//! lopdf 0.33 supports named encodings, Tj and TJ; it does not implement Identity-H
//! or ToUnicode CMaps. Fail explicitly rather than returning its placeholder text.
use lopdf::{
    content::{Content, Operation},
    Document, Object, ObjectId, Stream,
};
use std::{io::Read, path::Path};

const MAX_INPUT_BYTES: usize = 20 * 1024 * 1024;
const MAX_PAGES: usize = 100;
const MAX_PAGE_CONTENT_BYTES: usize = 2 * 1024 * 1024;
const MAX_CONTENT_BYTES: usize = 16 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 8000;

pub(crate) fn extract_pdf_text(path: &Path) -> Result<String, String> {
    // Take bounds the read even if the file grows after it is opened.
    let file = std::fs::File::open(path).map_err(|e| format!("Failed to open PDF: {e}"))?;
    let mut bytes = Vec::new();
    file.take((MAX_INPUT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Failed to read PDF: {e}"))?;
    if bytes.len() > MAX_INPUT_BYTES {
        return Err("PDF exceeds the 20 MiB input limit; no text was returned.".into());
    }
    let pdf = Document::load_mem(&bytes).map_err(|e| format!("Failed to parse PDF: {e}"))?;
    extract_document(pdf)
}

fn extract_document(mut pdf: Document) -> Result<String, String> {
    if pdf.is_encrypted() {
        return Err(
            "Encrypted PDFs are not supported. No password or OCR fallback is attempted.".into(),
        );
    }
    validate_page_tree(&pdf)?;
    let pages = pdf.get_pages();
    if pages.is_empty() {
        return Err("PDF has no readable page tree.".into());
    }
    if pages.len() > MAX_PAGES {
        return Err(format!(
            "PDF exceeds the {MAX_PAGES}-page limit; no text was returned."
        ));
    }
    let mut output = String::new();
    let mut truncated = false;
    let mut empty_pages = Vec::new();
    let mut content_bytes = 0;
    for (number, id) in &pages {
        let result = (|| -> Result<String, String> {
            check_parent_chain(&pdf, *id)?;
            let page = pdf.get_dictionary(*id).map_err(|e| e.to_string())?;
            let mut bytes = Vec::new();
            if let Ok(contents) = page.get(b"Contents") {
                read_contents(&pdf, contents, 0, &mut bytes)?;
            }
            content_bytes += bytes.len();
            if content_bytes > MAX_CONTENT_BYTES {
                return Err("decoded content exceeds 16 MiB".into());
            }
            // nom_parser in lopdf 0.33 accepts a parsed prefix. A final sentinel
            // must survive decoding, otherwise trailing malformed content was skipped.
            bytes.extend_from_slice(b"\nUnoOneContentEnd\n");
            let mut content =
                Content::decode(&bytes).map_err(|e| format!("invalid content stream: {e}"))?;
            if content.operations.pop().map(|op| op.operator) != Some("UnoOneContentEnd".into()) {
                return Err(
                    "malformed or unsupported content stream (partial extraction refused)".into(),
                );
            }
            normalize_text_operations(&pdf, *id, &mut content)?;
            let normalized = content.encode().map_err(|e| e.to_string())?;
            // Avoid get_page_content's silent skip/fallback behavior: use only
            // the streams we resolved and decoded successfully above.
            let stream = pdf.add_object(Stream::new(lopdf::Dictionary::new(), normalized));
            pdf.get_object_mut(*id)
                .and_then(Object::as_dict_mut)
                .map_err(|e| e.to_string())?
                .set("Contents", stream);
            pdf.extract_text(&[*number])
                .map_err(|e| format!("text extraction failed: {e}"))
        })();
        let text =
            result.map_err(|e| format!("PDF page {number}: {e}. No partial text was returned."))?;
        if text.trim().is_empty() {
            empty_pages.push(number.to_string());
            continue;
        }
        let part = format!("[Page {number}]\n{}\n\n", text.trim());
        let remaining = MAX_TEXT_BYTES.saturating_sub(output.len());
        let head = unoone_text::truncate_bytes(&part, remaining);
        truncated |= head.len() != part.len();
        output.push_str(head);
        // Continue validating later pages even after the excerpt budget is full.
    }
    if output.is_empty() {
        return Err("No readable text layer was extracted. This PDF may be scanned, image-only or use unsupported text. OCR fallback is not implemented.".into());
    }
    if truncated {
        output.push_str("\n\n[Truncated — excerpt limited to 8,000 UTF-8 bytes; this is not the complete document.]");
    }
    if !empty_pages.is_empty() {
        output.push_str(&format!(
            "\n\n[No readable text layer on pages {}. These pages were not OCR-processed.]",
            empty_pages.join(", ")
        ));
    }
    output.push_str("\n\n[PDF text layer only; images are not OCR-processed. Reading order and layout may differ from the original.]");
    Ok(output)
}

// get_pages is permissive about broken Kids references; verify the tree first.
fn validate_page_tree(pdf: &Document) -> Result<(), String> {
    fn visit(
        pdf: &Document,
        id: ObjectId,
        depth: usize,
        seen: &mut std::collections::HashSet<ObjectId>,
        pages: &mut usize,
    ) -> Result<usize, String> {
        if depth > 64 || !seen.insert(id) {
            return Err("PDF page tree is cyclic or too deep.".into());
        }
        let node = pdf
            .get_dictionary(id)
            .map_err(|e| format!("Invalid PDF page tree: {e}"))?;
        let kind = node
            .get(b"Type")
            .and_then(Object::as_name_str)
            .map_err(|e| e.to_string())?;
        if kind == "Page" {
            *pages += 1;
            if *pages > MAX_PAGES {
                return Err(format!(
                    "PDF exceeds the {MAX_PAGES}-page limit; no text was returned."
                ));
            }
            return Ok(1);
        }
        if kind != "Pages" {
            return Err("Invalid PDF page tree node type.".into());
        }
        let kids = node
            .get(b"Kids")
            .and_then(Object::as_array)
            .map_err(|e| e.to_string())?;
        let mut count = 0;
        for kid in kids {
            count += visit(
                pdf,
                kid.as_reference().map_err(|e| e.to_string())?,
                depth + 1,
                seen,
                pages,
            )?;
        }
        if node
            .get(b"Count")
            .and_then(Object::as_i64)
            .map_err(|e| e.to_string())?
            != count as i64
        {
            return Err("PDF page tree count mismatch; no partial text was returned.".into());
        }
        Ok(count)
    }
    let root = pdf
        .catalog()
        .and_then(|c| c.get(b"Pages"))
        .and_then(Object::as_reference)
        .map_err(|e| format!("Invalid PDF page tree: {e}"))?;
    visit(pdf, root, 0, &mut std::collections::HashSet::new(), &mut 0)?;
    Ok(())
}

fn check_parent_chain(pdf: &Document, id: ObjectId) -> Result<(), String> {
    let mut next = Some(id);
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = next {
        if !seen.insert(id) || seen.len() > 64 {
            return Err("cyclic or excessively deep page parent chain".into());
        }
        let page = pdf.get_dictionary(id).map_err(|e| e.to_string())?;
        next = match page.get(b"Parent") {
            Ok(parent) => Some(parent.as_reference().map_err(|e| e.to_string())?),
            Err(_) => None,
        };
    }
    Ok(())
}

fn read_contents(
    pdf: &Document,
    object: &Object,
    depth: usize,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    if depth > 32 {
        return Err("cyclic or excessively deep content references".into());
    }
    match object {
        Object::Reference(id) => read_contents(
            pdf,
            pdf.get_object(*id).map_err(|e| e.to_string())?,
            depth + 1,
            out,
        )?,
        Object::Array(items) => {
            for item in items {
                read_contents(pdf, item, depth + 1, out)?;
            }
        }
        Object::Stream(stream) => {
            // lopdf has no bounded decompressor API. This checks the decoded
            // size before parsing, not peak allocation inside the dependency.
            let decoded = if stream.dict.has(b"Filter") {
                // lopdf 0.33 swallows some Flate/LZW decode errors. Support a
                // single ordinary Flate stream and verify its zlib checksum;
                // reject other filter chains rather than accepting partial bytes.
                if stream.filters().map_err(|e| e.to_string())? != ["FlateDecode"]
                    || stream.dict.has(b"DecodeParms")
                {
                    return Err(
                        "cannot decode content filter: only plain FlateDecode is supported".into(),
                    );
                }
                let decoded = stream
                    .decompressed_content()
                    .map_err(|e| format!("cannot decode content filter: {e}"))?;
                verify_zlib(&stream.content, &decoded)?;
                decoded
            } else {
                stream.content.clone()
            };
            if out.len().saturating_add(decoded.len()).saturating_add(1) > MAX_PAGE_CONTENT_BYTES {
                return Err("page content exceeds 2 MiB decoded limit".into());
            }
            out.extend(decoded);
            out.push(b'\n');
        }
        Object::Null => {}
        _ => return Err("page content is not a stream or stream array".into()),
    }
    Ok(())
}

fn verify_zlib(encoded: &[u8], decoded: &[u8]) -> Result<(), String> {
    if encoded.len() < 6
        || encoded[0] & 15 != 8
        || encoded[1] & 32 != 0
        || (u16::from(encoded[0]) * 256 + u16::from(encoded[1])) % 31 != 0
    {
        return Err("invalid Flate/zlib header".into());
    }
    let (mut a, mut b) = (1u32, 0u32);
    for byte in decoded {
        a = (a + u32::from(*byte)) % 65521;
        b = (b + a) % 65521;
    }
    let expected = u32::from_be_bytes(
        encoded[encoded.len() - 4..]
            .try_into()
            .map_err(|_| "missing Flate checksum")?,
    );
    if expected != (b << 16 | a) {
        return Err("Flate checksum mismatch (partial decompression refused)".into());
    }
    Ok(())
}

fn normalize_text_operations(
    pdf: &Document,
    id: ObjectId,
    content: &mut Content<Vec<Operation>>,
) -> Result<(), String> {
    let fonts = pdf.get_page_fonts(id);
    let mut selected_font = None;
    let mut normalized = Vec::new();
    for mut op in std::mem::take(&mut content.operations) {
        match op.operator.as_str() {
            "T*" => normalized.push(Operation::new("ET", vec![])),
            "Td" | "TD" => {
                if op.operands.len() != 2 {
                    return Err("invalid text-position operands".into());
                }
                if op.operands[1].as_float().map_err(|e| e.to_string())? != 0.0 {
                    // extract_text ignores vertical positioning. Supply a line
                    // separator without claiming exact original layout.
                    normalized.push(Operation::new("ET", vec![]));
                }
            }
            "Tf" => {
                if op.operands.len() != 2 {
                    return Err("invalid Tf operands".into());
                }
                let name = op.operands[0].as_name().map_err(|e| e.to_string())?;
                selected_font = Some(*fonts.get(name).ok_or("text font resource is missing")?);
            }
            "Tj" | "TJ" | "'" | "\"" => {
                let font = selected_font.ok_or("text has no selected font")?;
                let encoding = font.get_font_encoding();
                if font.has(b"ToUnicode")
                    || font.get(b"Encoding").is_ok_and(|e| e.as_name().is_err())
                    || !matches!(
                        encoding,
                        "StandardEncoding"
                            | "MacRomanEncoding"
                            | "MacExpertEncoding"
                            | "WinAnsiEncoding"
                            | "UniGB-UCS2-H"
                            | "UniGB−UTF16−H"
                    )
                {
                    return Err(format!("unsupported font encoding/CMap ({encoding}); this extractor does not implement ToUnicode or Identity-H"));
                }
                let count = if op.operator == "\"" { 3 } else { 1 };
                if op.operands.len() != count {
                    return Err("invalid text operator operands".into());
                }
                let text = op.operands.last().ok_or("missing text operand")?;
                if op.operator == "TJ" {
                    let items = text.as_array().map_err(|e| e.to_string())?;
                    if !items.iter().all(|item| {
                        matches!(
                            item,
                            Object::String(_, _) | Object::Integer(_) | Object::Real(_)
                        )
                    }) {
                        return Err("invalid TJ array".into());
                    }
                } else {
                    text.as_str().map_err(|e| e.to_string())?;
                }
                if op.operator == "'" || op.operator == "\"" {
                    normalized.push(Operation::new("ET", vec![]));
                    op = Operation::new("Tj", vec![text.clone()]);
                }
            }
            "Do" => {
                // Form XObjects can contain text that extract_text does not visit.
                // Reject them rather than silently omitting it. Images are disclosed.
                let name = op
                    .operands
                    .first()
                    .ok_or("missing XObject name")?
                    .as_name()
                    .map_err(|e| e.to_string())?;
                let (local, parents) = pdf.get_page_resources(id);
                let resources = local
                    .into_iter()
                    .chain(parents.iter().filter_map(|id| pdf.get_dictionary(*id).ok()));
                let mut image = false;
                for resource in resources {
                    if let Ok(xobjects) = resource
                        .get(b"XObject")
                        .and_then(|o| pdf.dereference(o).map(|(_, o)| o))
                        .and_then(Object::as_dict)
                    {
                        if let Ok(object) = xobjects
                            .get(name)
                            .and_then(|o| pdf.dereference(o).map(|(_, o)| o))
                            .and_then(Object::as_stream)
                        {
                            image = object
                                .dict
                                .get(b"Subtype")
                                .and_then(Object::as_name_str)
                                .ok()
                                == Some("Image");
                            break;
                        }
                    }
                }
                if !image {
                    return Err(
                        "Form or unresolved XObject is not supported for text extraction".into(),
                    );
                }
            }
            _ => {}
        }
        normalized.push(op);
    }
    content.operations = normalized;
    Ok(())
}

#[cfg(test)]
#[path = "documents_pdf_tests.rs"]
mod tests;
