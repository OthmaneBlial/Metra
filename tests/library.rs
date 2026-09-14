use std::fs;
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::time::{SystemTime, UNIX_EPOCH};

struct ShortReader {
    inner: Cursor<Vec<u8>>,
    max_read: usize,
}

impl Read for ShortReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let end = buffer.len().min(self.max_read);
        self.inner.read(&mut buffer[..end])
    }
}

impl Seek for ShortReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.inner.seek(position)
    }
}

fn minimal_psd_with_xmp(format: &str) -> Vec<u8> {
    let xmp = format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF><rdf:Description xmlns:dc=\"urn:dc\" dc:format=\"{format}\"/></rdf:RDF></x:xmpmeta>"
    )
    .into_bytes();
    let mut resource = b"8BIM".to_vec();
    resource.extend_from_slice(&0x0424_u16.to_be_bytes());
    resource.extend_from_slice(&[0, 0]);
    resource.extend_from_slice(&(xmp.len() as u32).to_be_bytes());
    resource.extend_from_slice(&xmp);
    if xmp.len() % 2 == 1 {
        resource.push(0);
    }
    let mut bytes = vec![0_u8; 26];
    bytes[..4].copy_from_slice(b"8BPS");
    bytes[4..6].copy_from_slice(&1_u16.to_be_bytes());
    bytes[12..14].copy_from_slice(&3_u16.to_be_bytes());
    bytes[14..18].copy_from_slice(&100_u32.to_be_bytes());
    bytes[18..22].copy_from_slice(&200_u32.to_be_bytes());
    bytes[22..24].copy_from_slice(&8_u16.to_be_bytes());
    bytes[24..26].copy_from_slice(&3_u16.to_be_bytes());
    bytes.extend_from_slice(&0_u32.to_be_bytes());
    bytes.extend_from_slice(&(resource.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&resource);
    bytes.extend_from_slice(&0_u32.to_be_bytes());
    bytes.extend_from_slice(&0_u16.to_be_bytes());
    bytes
}

fn minimal_avi_with_title(title: &str) -> Vec<u8> {
    let mut info_chunk = b"INAM".to_vec();
    info_chunk.extend_from_slice(&(title.len() as u32).to_le_bytes());
    info_chunk.extend_from_slice(title.as_bytes());
    if title.len() % 2 == 1 {
        info_chunk.push(0);
    }
    let mut info_payload = b"INFO".to_vec();
    info_payload.extend_from_slice(&info_chunk);
    let mut info_list = b"LIST".to_vec();
    info_list.extend_from_slice(&(info_payload.len() as u32).to_le_bytes());
    info_list.extend_from_slice(&info_payload);
    if info_payload.len() % 2 == 1 {
        info_list.push(0);
    }
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&((4 + info_list.len()) as u32).to_le_bytes());
    bytes.extend_from_slice(b"AVI ");
    bytes.extend_from_slice(&info_list);
    bytes
}

fn minimal_webm_with_title(title: &str) -> Vec<u8> {
    fn element(id: &[u8], data: &[u8]) -> Vec<u8> {
        assert!(data.len() < 127);
        let mut output = id.to_vec();
        output.push(0x80 | data.len() as u8);
        output.extend_from_slice(data);
        output
    }

    let ebml_header = element(&[0x1A, 0x45, 0xDF, 0xA3], &element(&[0x42, 0x82], b"webm"));
    let simple_tag = [
        element(&[0x45, 0xA3], b"TITLE"),
        element(&[0x44, 0x87], title.as_bytes()),
    ]
    .concat();
    let tags = element(
        &[0x12, 0x54, 0xC3, 0x67],
        &element(&[0x73, 0x73], &element(&[0x67, 0xC8], &simple_tag)),
    );
    [ebml_header, tags].concat()
}

fn minimal_webm_with_info_title(title: &str) -> Vec<u8> {
    fn element(id: &[u8], data: &[u8]) -> Vec<u8> {
        assert!(data.len() < 127);
        let mut output = id.to_vec();
        output.push(0x80 | data.len() as u8);
        output.extend_from_slice(data);
        output
    }

    let ebml_header = element(&[0x1A, 0x45, 0xDF, 0xA3], &element(&[0x42, 0x82], b"webm"));
    let info = element(
        &[0x15, 0x49, 0xA9, 0x66],
        &element(&[0x7B, 0xA9], title.as_bytes()),
    );
    [ebml_header, info].concat()
}

fn minimal_cr3_with_title(title: &str) -> Vec<u8> {
    fn box_with_kind(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let size = u32::try_from(data.len() + 8).expect("test box fits");
        let mut output = size.to_be_bytes().to_vec();
        output.extend_from_slice(kind);
        output.extend_from_slice(data);
        output
    }

    let ftyp = box_with_kind(b"ftyp", b"crx \0\0\0\0crx ");
    let data = box_with_kind(
        b"data",
        &[&[0, 0, 0, 1, 0, 0, 0, 0], title.as_bytes()].concat(),
    );
    let title_kind = [0xA9, b'n', b'a', b'm'];
    let title = box_with_kind(&title_kind, &data);
    let ilst = box_with_kind(b"ilst", &title);
    let udta = box_with_kind(b"udta", &ilst);
    let moov = box_with_kind(b"moov", &udta);
    [ftyp, moov].concat()
}

fn minimal_xmp_packet(format: &str) -> Vec<u8> {
    format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF><rdf:Description xmlns:dc=\"urn:dc\" dc:format=\"{format}\"/></rdf:RDF></x:xmpmeta>"
    )
    .into_bytes()
}

fn minimal_isobmff_xmp(format: &str) -> Vec<u8> {
    fn box_with_kind(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let size = u32::try_from(data.len() + 8).expect("test box fits");
        let mut output = size.to_be_bytes().to_vec();
        output.extend_from_slice(kind);
        output.extend_from_slice(data);
        output
    }

    [
        box_with_kind(b"ftyp", b"isom\0\0\0\0"),
        box_with_kind(b"xml ", &minimal_xmp_packet(format)),
    ]
    .concat()
}

fn minimal_dng_with_make(make: &str) -> Vec<u8> {
    let mut bytes = vec![
        b'I',
        b'I',
        42,
        0,
        8,
        0,
        0,
        0,
        1,
        0, // one IFD0 entry
        0x0F,
        0x01,
        2,
        0,
        (make.len() as u16).to_le_bytes()[0],
        (make.len() as u16).to_le_bytes()[1],
        0,
        0,
        26,
        0,
        0,
        0,
        0,
        0,
        0,
        0, // no next IFD
    ];
    assert_eq!(bytes.len(), 26);
    bytes.extend_from_slice(make.as_bytes());
    bytes
}

fn minimal_tiff_with_gps() -> Vec<u8> {
    let mut bytes = vec![
        b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, // one IFD0 entry
        0x25, 0x88, 4, 0, 1, 0, 0, 0, 26, 0, 0, 0, // GPS IFD -> offset 26
        0, 0, 0, 0, // no next IFD
        2, 0, // two GPS IFD entries
        2, 0, 5, 0, 3, 0, 0, 0, 56, 0, 0, 0, // GPSLatitude -> offset 56
        1, 0, 2, 0, 2, 0, 0, 0, b'N', 0, 0, 0, // GPSLatitudeRef = N
        0, 0, 0, 0, // no next IFD
    ];
    bytes.extend_from_slice(&48_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    bytes.extend_from_slice(&51_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    bytes.extend_from_slice(&24_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    assert_eq!(bytes.len(), 80);
    bytes
}

#[test]
fn public_reader_api_detects_and_dispatches_in_memory_tiff() {
    let bytes = b"II*\0\0\0\0\0";
    let mut reader = Cursor::new(bytes.as_slice());
    let metadata = metra::read_from(
        &mut reader,
        metra::FileInfo::new(
            "memory.tif".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
    )
    .expect("in-memory TIFF should use the public reader dispatch");

    assert_eq!(metadata.file_info.format, metra::FileFormat::Tiff);
    assert!(
        metadata
            .warnings
            .iter()
            .any(|warning| warning.code == "missing-ifd")
    );
}

#[test]
fn public_reader_api_handles_short_signature_reads() {
    let bytes = b"II*\0\0\0\0\0".to_vec();
    let mut reader = ShortReader {
        inner: Cursor::new(bytes.clone()),
        max_read: 1,
    };
    let metadata = metra::read_from(
        &mut reader,
        metra::FileInfo::new(
            "short-reader.tif".into(),
            bytes.len() as u64,
            metra::FileFormat::Jpeg,
        ),
    )
    .expect("short reads should not prevent signature detection");

    assert_eq!(metadata.file_info.format, metra::FileFormat::Tiff);
}

#[test]
fn public_format_registry_reports_every_read_dispatch_entry() {
    let handlers = metra::format_handlers();
    assert_eq!(handlers.len(), metra::format_capabilities_all().len());

    for capabilities in metra::format_capabilities_all() {
        let handler = metra::handler_for_format(capabilities.format)
            .expect("every advertised format should have a public handler");
        assert_eq!(handler.format(), capabilities.format);
        assert_eq!(handler.capabilities(), *capabilities);
    }
}

#[test]
fn public_generic_edit_api_rewrites_and_revalidates_jpeg() {
    let bytes = [
        0xFF, 0xD8, // SOI
        0xFF, 0xFE, 0x00, 0x05, b'o', b'l', b'd', // COM
        0xFF, 0xD9, // EOI
    ];
    let output = metra::rewrite_metadata_to_vec(
        &bytes,
        metra::FileInfo::new(
            "memory.jpg".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
        metra::ParseLimits::default(),
        &[metra::MetadataEdit::set("JPEG:Comment", "new")],
    )
    .expect("generic JPEG edit should validate its rewritten bytes");

    let metadata = metra::read_from(
        &mut std::io::Cursor::new(output),
        metra::FileInfo::new("memory.jpg".into(), 0, metra::FileFormat::Unknown),
    )
    .expect("rewritten JPEG should remain readable");
    assert_eq!(
        metadata.find("JPEG:Comment").unwrap().display_value(),
        "new"
    );
}

#[test]
fn public_generic_edit_api_rewrites_embedded_isobmff_xmp() {
    let bytes = minimal_isobmff_xmp("Before");
    let replacement = String::from_utf8(minimal_xmp_packet("After!")).expect("XMP is UTF-8");
    let output = metra::rewrite_metadata_to_vec(
        &bytes,
        metra::FileInfo::new(
            "memory.heic".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
        metra::ParseLimits::default(),
        &[metra::MetadataEdit::set("ISOBMFF:XMP", replacement)],
    )
    .expect("generic ISO-BMFF XMP edit should validate its rewritten bytes");

    let metadata = metra::read_from(
        &mut std::io::Cursor::new(output.clone()),
        metra::FileInfo::new(
            "memory.heic".into(),
            output.len() as u64,
            metra::FileFormat::Unknown,
        ),
    )
    .expect("rewritten ISO-BMFF should remain readable");
    assert_eq!(
        metadata.find("XMP:dc:format").unwrap().display_value(),
        "After!"
    );
}

#[test]
fn public_generic_edit_api_rewrites_and_revalidates_pdf_info() {
    let bytes = b"%PDF-1.7\n5 0 obj\n<< /Title (Before) >>\nendobj\ntrailer\n<< /Info 5 0 R >>\nstartxref\n9\n%%EOF\n";
    let output = metra::rewrite_metadata_to_vec(
        bytes,
        metra::FileInfo::new(
            "memory.pdf".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
        metra::ParseLimits::default(),
        &[metra::MetadataEdit::set("PDF:Title", "After!")],
    )
    .expect("generic PDF edit should validate its rewritten bytes");

    let metadata = metra::read_from(
        &mut std::io::Cursor::new(output),
        metra::FileInfo::new(
            "memory.pdf".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
    )
    .expect("rewritten PDF should remain readable");
    assert_eq!(
        metadata.find("PDF:Title").unwrap().display_value(),
        "After!"
    );
}

#[test]
fn public_generic_edit_api_deletes_and_revalidates_pdf_info() {
    let bytes = b"%PDF-1.7\n5 0 obj\n<< /Title (Before) /Author (Ot) >>\nendobj\ntrailer\n<< /Info 5 0 R >>\nstartxref\n9\n%%EOF\n";
    let output = metra::rewrite_metadata_to_vec(
        bytes,
        metra::FileInfo::new(
            "memory.pdf".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
        metra::ParseLimits::default(),
        &[metra::MetadataEdit::delete("PDF:Title")],
    )
    .expect("generic PDF deletion should validate its rewritten bytes");

    assert_eq!(output.len(), bytes.len());
    let metadata = metra::read_from(
        &mut std::io::Cursor::new(output),
        metra::FileInfo::new(
            "memory.pdf".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
    )
    .expect("deleted PDF should remain readable");
    assert!(metadata.find("PDF:Title").is_none());
    assert_eq!(metadata.find("PDF:Author").unwrap().display_value(), "Ot");
}

#[test]
fn public_generic_edit_api_rewrites_and_revalidates_psd_xmp() {
    let bytes = minimal_psd_with_xmp("old");
    let xmp = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF><rdf:Description xmlns:dc=\"urn:dc\" dc:format=\"new\"/></rdf:RDF></x:xmpmeta>";
    let output = metra::rewrite_metadata_to_vec(
        &bytes,
        metra::FileInfo::new(
            "memory.psd".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
        metra::ParseLimits::default(),
        &[metra::MetadataEdit::set("PSD:XMP", xmp)],
    )
    .expect("generic PSD edit should validate its rewritten bytes");

    let metadata = metra::read_from(
        &mut std::io::Cursor::new(output),
        metra::FileInfo::new(
            "memory.psd".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
    )
    .expect("rewritten PSD should remain readable");
    assert_eq!(
        metadata.find("XMP:dc:format").unwrap().display_value(),
        "new"
    );
}

#[test]
fn public_generic_edit_api_deletes_and_revalidates_psd_xmp() {
    let bytes = minimal_psd_with_xmp("old");
    let output = metra::rewrite_metadata_to_vec(
        &bytes,
        metra::FileInfo::new(
            "memory.psd".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
        metra::ParseLimits::default(),
        &[metra::MetadataEdit::delete("PSD:XMP")],
    )
    .expect("generic PSD deletion should validate its rewritten bytes");

    assert_eq!(output.len(), bytes.len());
    let metadata = metra::read_from(
        &mut std::io::Cursor::new(output),
        metra::FileInfo::new(
            "memory.psd".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
    )
    .expect("deleted PSD should remain readable");
    assert!(metadata.find("XMP:Packet").is_none());
    assert!(metadata.warnings.is_empty());
}

#[test]
fn public_generic_edit_api_rewrites_and_revalidates_standalone_xmp() {
    let bytes = minimal_xmp_packet("old");
    let replacement = String::from_utf8(minimal_xmp_packet("new")).expect("XMP should be UTF-8");
    let output = metra::rewrite_metadata_to_vec(
        &bytes,
        metra::FileInfo::new(
            "memory.xmp".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
        metra::ParseLimits::default(),
        &[metra::MetadataEdit::set("XMP:Packet", replacement)],
    )
    .expect("generic standalone XMP edit should validate its rewritten bytes");

    assert_eq!(output.len(), bytes.len());
    let metadata = metra::read_from(
        &mut std::io::Cursor::new(output),
        metra::FileInfo::new(
            "memory.xmp".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
    )
    .expect("rewritten standalone XMP should remain readable");
    assert_eq!(
        metadata.find("XMP:dc:format").unwrap().display_value(),
        "new"
    );
}

#[test]
fn public_generic_edit_api_clears_standalone_xmp_properties() {
    let bytes = minimal_xmp_packet("old");
    let output = metra::rewrite_metadata_to_vec(
        &bytes,
        metra::FileInfo::new(
            "memory.xmp".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
        metra::ParseLimits::default(),
        &[metra::MetadataEdit::delete("XMP:Packet")],
    )
    .expect("generic standalone XMP deletion should validate its rewritten bytes");

    assert_eq!(output.len(), bytes.len());
    let metadata = metra::read_from(
        &mut std::io::Cursor::new(output),
        metra::FileInfo::new(
            "memory.xmp".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
    )
    .expect("cleared standalone XMP should remain readable");
    assert!(metadata.find("XMP:dc:format").is_none());
    assert!(metadata.find("XMP:Packet").is_some());
}

#[test]
fn public_generic_edit_api_rewrites_and_revalidates_icc_text() {
    let bytes = metra::create_icc_to_vec(
        &metra::IccCreateOptions::new().with_text("Description", "old"),
        metra::ParseLimits::default(),
    )
    .expect("ICC seed should be created");
    let output = metra::rewrite_metadata_to_vec(
        &bytes,
        metra::FileInfo::new(
            "memory.icc".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
        metra::ParseLimits::default(),
        &[metra::MetadataEdit::set("ICC:Description", "new")],
    )
    .expect("generic ICC edit should validate its rewritten bytes");

    assert_eq!(output.len(), bytes.len());
    let metadata = metra::read_from(
        &mut std::io::Cursor::new(output),
        metra::FileInfo::new(
            "memory.icc".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
    )
    .expect("rewritten ICC should remain readable");
    assert_eq!(
        metadata.find("ICC:Description").unwrap().display_value(),
        "new"
    );
}

#[test]
fn public_generic_edit_api_deletes_icc_text_without_resizing() {
    let bytes = metra::create_icc_to_vec(
        &metra::IccCreateOptions::new().with_text("Description", "old"),
        metra::ParseLimits::default(),
    )
    .expect("ICC seed should be created");
    let output = metra::rewrite_metadata_to_vec(
        &bytes,
        metra::FileInfo::new(
            "memory.icc".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
        metra::ParseLimits::default(),
        &[metra::MetadataEdit::delete("ICC:Description")],
    )
    .expect("generic ICC deletion should validate its rewritten bytes");

    assert_eq!(output.len(), bytes.len());
    let metadata = metra::read_from(
        &mut std::io::Cursor::new(output),
        metra::FileInfo::new(
            "memory.icc".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
    )
    .expect("deleted ICC should remain readable");
    assert!(metadata.find("ICC:Description").is_none());
}

#[test]
fn public_generic_edit_api_rewrites_and_revalidates_avi_info() {
    let bytes = minimal_avi_with_title("old");
    let output = metra::rewrite_metadata_to_vec(
        &bytes,
        metra::FileInfo::new(
            "memory.avi".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
        metra::ParseLimits::default(),
        &[metra::MetadataEdit::set("AVI:Title", "new")],
    )
    .expect("generic AVI edit should validate its rewritten bytes");

    let metadata = metra::read_from(
        &mut std::io::Cursor::new(output),
        metra::FileInfo::new(
            "memory.avi".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
    )
    .expect("rewritten AVI should remain readable");
    assert_eq!(metadata.find("AVI:Title").unwrap().display_value(), "new");
}

#[test]
fn public_generic_edit_api_deletes_existing_avi_info() {
    let bytes = minimal_avi_with_title("old");
    let output = metra::rewrite_metadata_to_vec(
        &bytes,
        metra::FileInfo::new(
            "memory.avi".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
        metra::ParseLimits::default(),
        &[metra::MetadataEdit::delete("AVI:Title")],
    )
    .expect("generic AVI deletion should validate its rewritten bytes");

    let metadata = metra::read_from(
        &mut std::io::Cursor::new(output),
        metra::FileInfo::new(
            "memory.avi".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
    )
    .expect("deleted AVI should remain readable");
    assert!(metadata.find("AVI:Title").is_none());
}

#[test]
fn public_generic_edit_api_rewrites_and_revalidates_matroska_tag() {
    let bytes = minimal_webm_with_title("old");
    let output = metra::rewrite_metadata_to_vec(
        &bytes,
        metra::FileInfo::new(
            "memory.webm".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
        metra::ParseLimits::default(),
        &[metra::MetadataEdit::set("Matroska:Tag:TITLE", "new")],
    )
    .expect("generic Matroska edit should validate its rewritten bytes");

    let metadata = metra::read_from(
        &mut std::io::Cursor::new(output),
        metra::FileInfo::new(
            "memory.webm".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
    )
    .expect("rewritten WebM should remain readable");
    assert_eq!(
        metadata.find("Matroska:Tag:TITLE").unwrap().display_value(),
        "new"
    );
}

#[test]
fn public_generic_edit_api_rewrites_and_revalidates_matroska_info_title() {
    let bytes = minimal_webm_with_info_title("old");
    let output = metra::rewrite_metadata_to_vec(
        &bytes,
        metra::FileInfo::new(
            "memory.webm".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
        metra::ParseLimits::default(),
        &[metra::MetadataEdit::set("Matroska:Title", "new")],
    )
    .expect("generic Matroska Info edit should validate its rewritten bytes");

    let metadata = metra::read_from(
        &mut std::io::Cursor::new(output),
        metra::FileInfo::new(
            "memory.webm".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
    )
    .expect("rewritten WebM should remain readable");
    assert_eq!(
        metadata.find("Matroska:Title").unwrap().display_value(),
        "new"
    );
}

#[test]
fn public_generic_edit_api_deletes_existing_matroska_text() {
    let bytes = minimal_webm_with_info_title("old");
    let output = metra::rewrite_metadata_to_vec(
        &bytes,
        metra::FileInfo::new(
            "memory.webm".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
        metra::ParseLimits::default(),
        &[metra::MetadataEdit::delete("Matroska:Title")],
    )
    .expect("generic Matroska deletion should validate its rewritten bytes");

    let metadata = metra::read_from(
        &mut std::io::Cursor::new(output),
        metra::FileInfo::new(
            "memory.webm".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
    )
    .expect("deleted WebM should remain readable");
    assert!(metadata.find("Matroska:Title").is_none());
}

#[test]
fn public_generic_edit_api_rewrites_and_revalidates_tiff_like_raw() {
    let bytes = minimal_dng_with_make("Canon\0");
    let output = metra::rewrite_metadata_to_vec(
        &bytes,
        metra::FileInfo::new(
            "memory.dng".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
        metra::ParseLimits::default(),
        &[metra::MetadataEdit::set("TIFF:EXIF:Make", "Sony")],
    )
    .expect("generic TIFF-like RAW edit should validate its rewritten bytes");

    let metadata = metra::read_from(
        &mut std::io::Cursor::new(output),
        metra::FileInfo::new(
            "memory.dng".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
    )
    .expect("rewritten DNG should remain readable");
    assert_eq!(metadata.find("EXIF:Make").unwrap().display_value(), "Sony");
    assert_eq!(metadata.find("RAW:Variant").unwrap().display_value(), "DNG");
}

#[test]
fn public_generic_edit_api_deletes_existing_tiff_like_raw_ascii() {
    let bytes = minimal_dng_with_make("Canon\0");
    let output = metra::rewrite_metadata_to_vec(
        &bytes,
        metra::FileInfo::new(
            "memory.dng".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
        metra::ParseLimits::default(),
        &[metra::MetadataEdit::delete("TIFF:EXIF:Make")],
    )
    .expect("generic TIFF-like RAW deletion should validate its rewritten bytes");

    let metadata = metra::read_from(
        &mut std::io::Cursor::new(output),
        metra::FileInfo::new(
            "memory.dng".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
    )
    .expect("deleted DNG should remain readable");
    assert_eq!(metadata.find("RAW:Variant").unwrap().display_value(), "DNG");
    assert!(metadata.find("EXIF:Make").is_none());
}

#[test]
fn public_generic_edit_api_rewrites_gps_decimal_coordinates() {
    let bytes = minimal_tiff_with_gps();
    let output = metra::rewrite_metadata_to_vec(
        &bytes,
        metra::FileInfo::new(
            "memory.tif".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
        metra::ParseLimits::default(),
        &[metra::MetadataEdit::set("GPS:Latitude", "-48.8566")],
    )
    .expect("generic GPS edit should validate its rewritten bytes");

    let metadata = metra::read_from(
        &mut std::io::Cursor::new(output.clone()),
        metra::FileInfo::new(
            "memory.tif".into(),
            output.len() as u64,
            metra::FileFormat::Unknown,
        ),
    )
    .expect("rewritten GPS TIFF should remain readable");
    let latitude = metadata
        .find("GPS:LatitudeDecimal")
        .expect("derived latitude should be present")
        .value
        .clone();
    let metra::TagValue::Float(latitude) = latitude else {
        panic!("derived latitude should be a float");
    };
    assert!((latitude + 48.8566).abs() < 0.000001);
    assert_eq!(
        metadata.find("GPS:GPSLatitudeRef").unwrap().display_value(),
        "S"
    );
}

#[test]
fn public_generic_edit_api_rewrites_and_revalidates_cr3_text() {
    let bytes = minimal_cr3_with_title("old");
    let output = metra::rewrite_metadata_to_vec(
        &bytes,
        metra::FileInfo::new(
            "memory.cr3".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
        metra::ParseLimits::default(),
        &[metra::MetadataEdit::set("ISOBMFF:Title", "new")],
    )
    .expect("generic CR3 edit should validate its rewritten bytes");

    let metadata = metra::read_from(
        &mut std::io::Cursor::new(output),
        metra::FileInfo::new(
            "memory.cr3".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
    )
    .expect("rewritten CR3 should remain readable");
    assert_eq!(metadata.find("RAW:Variant").unwrap().display_value(), "CR3");
    assert_eq!(
        metadata.find("ISOBMFF:Title").unwrap().display_value(),
        "new"
    );
}

#[test]
fn public_generic_edit_api_deletes_existing_cr3_text() {
    let bytes = minimal_cr3_with_title("old");
    let output = metra::rewrite_metadata_to_vec(
        &bytes,
        metra::FileInfo::new(
            "memory.cr3".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
        metra::ParseLimits::default(),
        &[metra::MetadataEdit::delete("ISOBMFF:Title")],
    )
    .expect("generic CR3 deletion should validate its rewritten bytes");

    let metadata = metra::read_from(
        &mut std::io::Cursor::new(output),
        metra::FileInfo::new(
            "memory.cr3".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
    )
    .expect("deleted CR3 should remain readable");
    assert_eq!(metadata.find("RAW:Variant").unwrap().display_value(), "CR3");
    assert!(metadata.find("ISOBMFF:Title").is_none());
}

#[test]
fn public_generic_copy_api_reads_source_before_atomic_target_rewrite() {
    let source_bytes = [
        0xFF, 0xD8, 0xFF, 0xFE, 0x00, 0x0D, b'f', b'r', b'o', b'm', b' ', b's', b'o', b'u', b'r',
        b'c', b'e', 0xFF, 0xD9,
    ];
    let target_bytes = [
        0xFF, 0xD8, 0xFF, 0xFE, 0x00, 0x0C, b't', b'a', b'r', b'g', b'e', b't', b' ', b'o', b'l',
        b'd', 0xFF, 0xD9,
    ];
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after the Unix epoch")
        .as_nanos();
    let source = std::env::temp_dir().join(format!("metra-copy-source-{nonce}.jpg"));
    let target = std::env::temp_dir().join(format!("metra-copy-target-{nonce}.jpg"));
    fs::write(&source, source_bytes).expect("source fixture should be writable");
    fs::write(&target, target_bytes).expect("target fixture should be writable");

    metra::copy_metadata_path(
        &source,
        &target,
        metra::ParseLimits::default(),
        "JPEG:Comment",
    )
    .expect("generic copy should rewrite the target atomically");

    let metadata = metra::read(&target).expect("copied target should remain readable");
    assert_eq!(
        metadata.find("JPEG:Comment").unwrap().display_value(),
        "from source"
    );
    fs::remove_file(source).expect("source fixture should be removable");
    fs::remove_file(target).expect("target fixture should be removable");
}

#[test]
fn public_generic_copy_api_rewrites_standalone_xmp() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after the Unix epoch")
        .as_nanos();
    let source = std::env::temp_dir().join(format!("metra-copy-source-{nonce}.xmp"));
    let target = std::env::temp_dir().join(format!("metra-copy-target-{nonce}.xmp"));
    fs::write(&source, minimal_xmp_packet("source")).expect("source fixture should be writable");
    fs::write(&target, minimal_xmp_packet("target")).expect("target fixture should be writable");

    metra::copy_metadata_path(
        &source,
        &target,
        metra::ParseLimits::default(),
        "XMP:Packet",
    )
    .expect("generic standalone XMP copy should rewrite the target");

    let metadata = metra::read(&target).expect("copied XMP should remain readable");
    assert_eq!(
        metadata.find("XMP:dc:format").unwrap().display_value(),
        "source"
    );
    fs::remove_file(source).expect("source fixture should be removable");
    fs::remove_file(target).expect("target fixture should be removable");
}

#[test]
fn public_generic_copy_api_rewrites_icc_text() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after the Unix epoch")
        .as_nanos();
    let source = std::env::temp_dir().join(format!("metra-copy-source-{nonce}.icc"));
    let target = std::env::temp_dir().join(format!("metra-copy-target-{nonce}.icc"));
    let source_bytes = metra::create_icc_to_vec(
        &metra::IccCreateOptions::new().with_text("Description", "source"),
        metra::ParseLimits::default(),
    )
    .expect("source ICC fixture should be creatable");
    let target_bytes = metra::create_icc_to_vec(
        &metra::IccCreateOptions::new().with_text("Description", "target"),
        metra::ParseLimits::default(),
    )
    .expect("target ICC fixture should be creatable");
    fs::write(&source, source_bytes).expect("source fixture should be writable");
    fs::write(&target, target_bytes).expect("target fixture should be writable");

    metra::copy_metadata_path(
        &source,
        &target,
        metra::ParseLimits::default(),
        "ICC:Description",
    )
    .expect("generic ICC copy should rewrite the target");

    let metadata = metra::read(&target).expect("copied ICC should remain readable");
    assert_eq!(
        metadata.find("ICC:Description").unwrap().display_value(),
        "source"
    );
    fs::remove_file(source).expect("source fixture should be removable");
    fs::remove_file(target).expect("target fixture should be removable");
}

#[test]
fn public_batch_api_keeps_input_order_and_supports_streaming() {
    let paths = vec![
        std::env::temp_dir().join("metra-batch-z-does-not-exist"),
        std::env::temp_dir().join("metra-batch-a-does-not-exist"),
        std::env::temp_dir().join("metra-batch-m-does-not-exist"),
    ];
    let options = metra::BatchOptions {
        jobs: 3,
        limits: metra::ParseLimits::default(),
    };

    let results = metra::read_many(&paths, options);
    assert_eq!(
        results.iter().map(|item| &item.path).collect::<Vec<_>>(),
        paths.iter().collect::<Vec<_>>()
    );
    assert!(results.iter().all(|item| item.result.is_err()));

    let mut streamed_paths = Vec::new();
    metra::read_many_streaming(&paths, options, |item| {
        streamed_paths.push(item.path);
    });
    assert_eq!(streamed_paths, paths);
}

#[test]
fn public_batch_api_cancels_without_opening_remaining_paths() {
    let paths = vec![
        std::env::temp_dir().join("metra-cancel-a-does-not-exist"),
        std::env::temp_dir().join("metra-cancel-b-does-not-exist"),
    ];
    let cancellation = metra::CancellationToken::new();
    cancellation.cancel();
    let options = metra::BatchOptions {
        jobs: 2,
        limits: metra::ParseLimits::default(),
    };

    let results = metra::read_many_with_cancellation(&paths, options, &cancellation);
    assert_eq!(results.len(), paths.len());
    assert!(
        results
            .iter()
            .all(|item| matches!(item.result, Err(metra::MetraError::Cancelled)))
    );

    let mut streamed = Vec::new();
    metra::read_many_streaming_with_cancellation(&paths, options, &cancellation, |item| {
        streamed.push(item);
    });
    assert_eq!(streamed.len(), paths.len());
    assert!(
        streamed
            .iter()
            .all(|item| matches!(item.result, Err(metra::MetraError::Cancelled)))
    );
}
