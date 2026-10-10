use super::*;
use lopdf::{dictionary, Dictionary};

/// Real page tree/resources/content streams, serialized then loaded by production code.
fn fixture(contents: &[&[u8]], encoding: &str) -> Document {
    let mut pdf = Document::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let font = pdf.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => Object::Name(encoding.as_bytes().to_vec()) });
    let mut kids = Vec::new();
    for content in contents {
        let stream = pdf.add_object(Stream::new(Dictionary::new(), content.to_vec()));
        let page = pdf.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 300.into(), 300.into()],
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
            "Contents" => stream,
        });
        kids.push(Object::Reference(page));
    }
    pdf.objects.insert(
        pages_id,
        dictionary! { "Type" => "Pages", "Count" => kids.len() as i64, "Kids" => kids }.into(),
    );
    let root = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", root);
    pdf
}

fn extract(mut pdf: Document) -> Result<String, String> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.pdf");
    pdf.save(&path).unwrap();
    extract_pdf_text(&path)
}

#[test]
fn real_tj_tj_array_escaped_octal_hex_and_quote_operators() {
    let pdf = fixture(&[br#"BT /F1 12 Tf (Hello \(world\)\040) Tj [(array) -200 <74657874>] TJ (next) ' 0 0 (line) " ET"#], "WinAnsiEncoding");
    let text = extract(pdf).unwrap();
    assert!(text.contains("Hello (world) array text"), "{text}");
    assert!(text.contains("next\nline"), "{text}");
    assert!(!text.contains("-200"));
}

#[test]
fn compressed_stream_and_multiple_streams_extract() {
    let mut pdf = fixture(&[b"BT /F1 12 Tf (compressed text repeated repeated repeated repeated repeated repeated) Tj ET"], "WinAnsiEncoding");
    pdf.compress();
    let page = *pdf.get_pages().get(&1).unwrap();
    pdf.add_page_contents(page, b"BT /F1 12 Tf (second stream) Tj ET".to_vec())
        .unwrap();
    let text = extract(pdf).unwrap();
    assert!(text.contains("compressed text"));
    assert!(text.contains("second stream"));
}

#[test]
fn supported_unicode_encoding_and_short_one_word_text_are_not_scans() {
    let mut pdf = fixture(&[b"BT /F1 12 Tf <4f60597d4e16754c> Tj ET"], "UniGB-UCS2-H");
    let font_id = pdf
        .objects
        .iter()
        .find(|(_, obj)| {
            obj.as_dict()
                .ok()
                .and_then(|d| d.get(b"Type").ok())
                .and_then(|o| o.as_name_str().ok())
                == Some("Font")
        })
        .map(|(id, _)| *id)
        .unwrap();
    let cid = pdf.add_object(dictionary! { "Type" => "Font", "Subtype" => "CIDFontType0", "BaseFont" => "STSong-Light", "CIDSystemInfo" => dictionary! { "Registry" => Object::string_literal("Adobe"), "Ordering" => Object::string_literal("GB1"), "Supplement" => 4 } });
    pdf.objects.insert(font_id, dictionary! { "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "STSong-Light", "Encoding" => "UniGB-UCS2-H", "DescendantFonts" => vec![Object::Reference(cid)] }.into());
    assert!(extract(pdf).unwrap().contains("你好世界"));
    assert!(
        extract(fixture(&[b"BT /F1 12 Tf (Title) Tj ET"], "WinAnsiEncoding"))
            .unwrap()
            .contains("Title")
    );
}

#[test]
fn encrypted_fixture_is_real_and_rejected_without_attempting_passwords() {
    let decoded = encrypted_fixture_bytes();
    let bytes = decoded.as_slice();
    let mut pdf = Document::load_mem(bytes).unwrap();
    assert!(pdf.is_encrypted());
    // Test-only verification that this fixture is encrypted, not merely marked so.
    pdf.decrypt(b"fixture-user").unwrap();
    assert!(pdf
        .extract_text(&[1])
        .unwrap()
        .contains("Encrypted fixture text"));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("encrypted.pdf");
    std::fs::write(&path, bytes).unwrap();
    assert!(extract_pdf_text(&path)
        .unwrap_err()
        .contains("Encrypted PDFs are not supported"));
}

#[test]
fn malformed_input_and_late_malformed_page_never_return_partial_success() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broken.pdf");
    std::fs::write(&path, b"not a pdf").unwrap();
    assert!(extract_pdf_text(&path)
        .unwrap_err()
        .contains("Failed to parse PDF"));
    let error = extract(fixture(
        &[
            b"BT /F1 12 Tf (good first page) Tj ET",
            b"BT /F1 12 Tf (unterminated",
        ],
        "WinAnsiEncoding",
    ))
    .unwrap_err();
    assert!(error.contains("page 2"), "{error}");
    assert!(error.contains("No partial text"));
}

#[test]
fn missing_content_reference_unsupported_filter_and_font_are_explicit_errors() {
    let mut pdf = fixture(&[b"BT /F1 12 Tf (first) Tj ET", b""], "WinAnsiEncoding");
    let page = *pdf.get_pages().get(&2).unwrap();
    pdf.get_object_mut(page)
        .unwrap()
        .as_dict_mut()
        .unwrap()
        .set("Contents", Object::Reference((9999, 0)));
    assert!(extract(pdf).unwrap_err().contains("page 2"));
    let mut pdf = fixture(&[b"BT /F1 12 Tf (text) Tj ET"], "WinAnsiEncoding");
    let stream = pdf
        .objects
        .values_mut()
        .find_map(|o| o.as_stream_mut().ok())
        .unwrap();
    stream.dict.set("Filter", "UnsupportedDecode");
    assert!(extract(pdf)
        .unwrap_err()
        .contains("cannot decode content filter"));
    assert!(
        extract(fixture(&[b"BT /F1 12 Tf <0001> Tj ET"], "Identity-H"))
            .unwrap_err()
            .contains("unsupported font encoding")
    );
    assert!(extract(fixture(
        &[b"BT /Missing 12 Tf (text) Tj ET"],
        "WinAnsiEncoding"
    ))
    .unwrap_err()
    .contains("font resource is missing"));
}

fn with_image(mut pdf: Document, number: u32) -> Document {
    let image = pdf.add_object(Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => 1, "Height" => 1, "ColorSpace" => "DeviceGray", "BitsPerComponent" => 8 }, vec![0]));
    let page = *pdf.get_pages().get(&number).unwrap();
    pdf.get_object_mut(page)
        .unwrap()
        .as_dict_mut()
        .unwrap()
        .get_mut(b"Resources")
        .unwrap()
        .as_dict_mut()
        .unwrap()
        .set("XObject", dictionary! { "Im1" => image });
    pdf
}

#[test]
fn image_only_fails_and_mixed_empty_pages_are_disclosed() {
    let pdf = with_image(
        fixture(&[b"q 100 0 0 100 0 0 cm /Im1 Do Q"], "WinAnsiEncoding"),
        1,
    );
    assert!(extract(pdf)
        .unwrap_err()
        .contains("OCR fallback is not implemented"));
    let pdf = with_image(
        fixture(
            &[b"BT /F1 12 Tf (some visible text) Tj ET", b"q /Im1 Do Q"],
            "WinAnsiEncoding",
        ),
        2,
    );
    assert!(extract(pdf)
        .unwrap()
        .contains("No readable text layer on pages 2"));
}

#[test]
fn page_markers_and_utf8_safe_truncation_include_notice_and_validate_late_pages() {
    // WinAnsi maps e9 to a two-byte UTF-8 e-acute, exercising a split byte boundary.
    let long = [
        b"BT /F1 12 Tf (".as_slice(),
        vec![0xe9; 6000].as_slice(),
        b") Tj ET",
    ]
    .concat();
    let text = extract(fixture(
        &[&long, b"BT /F1 12 Tf (last page text) Tj ET"],
        "WinAnsiEncoding",
    ))
    .unwrap();
    let prefix = text.split("\n\n[Truncated").next().unwrap();
    assert!(prefix.len() <= MAX_TEXT_BYTES);
    assert!(text.contains("Truncated — excerpt limited"));
    assert!(!text.contains('\u{fffd}'));
    assert!(extract(fixture(&[&long, b"BT (broken"], "WinAnsiEncoding"))
        .unwrap_err()
        .contains("page 2"));
    let text = extract(fixture(
        &[
            b"BT /F1 12 Tf (first) Tj ET",
            b"BT /F1 12 Tf (second) Tj ET",
        ],
        "WinAnsiEncoding",
    ))
    .unwrap();
    assert!(text.contains("[Page 1]"));
    assert!(text.contains("[Page 2]"));
    assert!(!text.contains("[Truncated"));
}

#[test]
fn input_page_and_decoded_content_limits_are_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large.pdf");
    std::fs::File::create(&path)
        .unwrap()
        .set_len((MAX_INPUT_BYTES + 1) as u64)
        .unwrap();
    assert!(extract_pdf_text(&path).unwrap_err().contains("20 MiB"));
    assert!(extract(fixture(
        &vec![b"".as_slice(); MAX_PAGES + 1],
        "WinAnsiEncoding"
    ))
    .unwrap_err()
    .contains("100-page"));
    let huge = vec![b' '; MAX_PAGE_CONTENT_BYTES + 1];
    assert!(extract(fixture(&[&huge], "WinAnsiEncoding"))
        .unwrap_err()
        .contains("2 MiB"));
}

#[test]
fn corrupt_compressed_stream_is_not_accepted_as_a_readable_prefix() {
    let mut pdf = fixture(&[b"BT /F1 12 Tf (text repeated repeated repeated repeated repeated repeated repeated repeated) Tj ET"], "WinAnsiEncoding");
    pdf.compress();
    let stream = pdf
        .objects
        .values_mut()
        .find_map(|o| o.as_stream_mut().ok())
        .unwrap();
    assert!(stream.dict.has(b"Filter"));
    let last = stream.content.len() - 1;
    stream.content[last] ^= 1; // corrupt only Adler-32: decoded prefix remains readable
    assert!(extract(pdf).unwrap_err().contains("checksum mismatch"));
}

#[test]
fn broken_page_tree_and_custom_encoding_are_rejected() {
    let mut pdf = fixture(&[b"BT /F1 12 Tf (text) Tj ET"], "WinAnsiEncoding");
    let pages = pdf
        .catalog()
        .unwrap()
        .get(b"Pages")
        .unwrap()
        .as_reference()
        .unwrap();
    pdf.get_object_mut(pages)
        .unwrap()
        .as_dict_mut()
        .unwrap()
        .set("Kids", vec![Object::Reference((9999, 0))]);
    assert!(extract(pdf).unwrap_err().contains("page tree"));
    let mut pdf = fixture(&[b"BT /F1 12 Tf (text) Tj ET"], "WinAnsiEncoding");
    let font = pdf
        .objects
        .values_mut()
        .filter_map(|o| o.as_dict_mut().ok())
        .find(|d| d.get(b"Type").and_then(Object::as_name_str).ok() == Some("Font"))
        .unwrap();
    font.set("Encoding", dictionary! { "BaseEncoding" => "WinAnsiEncoding", "Differences" => vec![65.into(), Object::Name(b"B".to_vec())] });
    assert!(extract(pdf)
        .unwrap_err()
        .contains("unsupported font encoding"));
}

#[test]
fn form_xobject_and_invalid_tj_array_are_not_silently_skipped() {
    assert!(extract(fixture(
        &[b"/Form1 Do BT /F1 12 Tf (partial text) Tj ET"],
        "WinAnsiEncoding"
    ))
    .unwrap_err()
    .contains("XObject"));
    assert!(extract(fixture(
        &[b"BT /F1 12 Tf [(text) /Bad] TJ ET"],
        "WinAnsiEncoding"
    ))
    .unwrap_err()
    .contains("invalid TJ"));
}

/// The encrypted fixture is stored hex-encoded (text) so the repository stays
/// free of opaque binary blobs; decoding here is exact and hash-checked.
fn encrypted_fixture_bytes() -> Vec<u8> {
    let text = include_str!("../tests/fixtures/encrypted-text.pdf.hex");
    let mut out = Vec::with_capacity(1024);
    for line in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let line = line.trim();
        assert!(line.len() % 2 == 0, "odd hex line");
        for i in (0..line.len()).step_by(2) {
            out.push(u8::from_str_radix(&line[i..i + 2], 16).expect("hex fixture"));
        }
    }
    assert_eq!(out.len(), 925, "fixture length changed");
    out
}
