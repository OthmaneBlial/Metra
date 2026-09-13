use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

struct TemporaryDirectory {
    path: PathBuf,
}

static NEXT_DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

impl TemporaryDirectory {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after the Unix epoch")
            .as_nanos();
        let counter = NEXT_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "metra-cli-test-{}-{unique}-{counter}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary test directory should be creatable");
        Self { path }
    }

    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.path.join(name);
        fs::write(&path, bytes).expect("fixture should be writable");
        path
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn minimal_exif_jpeg() -> Vec<u8> {
    let mut tiff = vec![
        b'I', b'I', 42, 0, 8, 0, 0, 0, // little-endian TIFF header
        1, 0, // one IFD0 entry
        0x0F, 0x01, 2, 0, 5, 0, 0, 0, 26, 0, 0, 0, // Make -> offset 26
        0, 0, 0, 0, // no next IFD
    ];
    assert_eq!(tiff.len(), 26);
    tiff.extend_from_slice(b"Sony\0");

    let mut exif = b"Exif\0\0".to_vec();
    exif.extend_from_slice(&tiff);
    let segment_length = u16::try_from(exif.len() + 2).expect("fixture fits in a JPEG segment");
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
    jpeg.extend_from_slice(&segment_length.to_be_bytes());
    jpeg.extend_from_slice(&exif);
    jpeg.extend_from_slice(&[0xFF, 0xD9]);
    jpeg
}

fn jpeg_iptc_dataset(dataset: u8, value: &str) -> Vec<u8> {
    let value = value.as_bytes();
    let mut bytes = vec![0x1C, 2, dataset];
    bytes.extend_from_slice(&(value.len() as u16).to_be_bytes());
    bytes.extend_from_slice(value);
    bytes
}

fn jpeg_iptc_resource(datasets: &[Vec<u8>]) -> Vec<u8> {
    let mut iptc = Vec::new();
    for dataset in datasets {
        iptc.extend_from_slice(dataset);
    }

    let mut resource = b"Photoshop 3.0\0".to_vec();
    resource.extend_from_slice(b"8BIM");
    resource.extend_from_slice(&0x0404_u16.to_be_bytes());
    resource.extend_from_slice(&[0, 0]);
    resource.extend_from_slice(&(iptc.len() as u32).to_be_bytes());
    resource.extend_from_slice(&iptc);
    if iptc.len() & 1 == 1 {
        resource.push(0);
    }
    resource
}

fn minimal_iptc_jpeg(keywords: &[&str], caption: &str) -> Vec<u8> {
    let mut datasets = keywords
        .iter()
        .map(|value| jpeg_iptc_dataset(25, value))
        .collect::<Vec<_>>();
    datasets.push(jpeg_iptc_dataset(120, caption));
    let app13 = jpeg_iptc_resource(&datasets);
    let segment_length = u16::try_from(app13.len() + 2).expect("fixture fits in a JPEG segment");
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xED];
    jpeg.extend_from_slice(&segment_length.to_be_bytes());
    jpeg.extend_from_slice(&app13);
    jpeg.extend_from_slice(&[0xFF, 0xD9]);
    jpeg
}

fn minimal_jpeg_xmp(format: &str) -> Vec<u8> {
    let packet = format!(
        "<x:xmpmeta><rdf:RDF><rdf:Description xmlns:dc=\"urn:dc\" dc:format=\"{format}\"/></rdf:RDF></x:xmpmeta>"
    );
    let mut app1 = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
    app1.extend_from_slice(packet.as_bytes());
    let segment_length = u16::try_from(app1.len() + 2).expect("fixture fits in a JPEG segment");
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
    jpeg.extend_from_slice(&segment_length.to_be_bytes());
    jpeg.extend_from_slice(&app1);
    jpeg.extend_from_slice(&[0xFF, 0xD9]);
    jpeg
}

fn minimal_svg() -> Vec<u8> {
    br#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" width="32" height="24"><title>CLI vector</title></svg>"#
        .to_vec()
}

fn minimal_svg_document(title: &str, description: &str, comment: &str) -> Vec<u8> {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\"><!-- {comment} --><title>{title}</title><desc>{description}</desc><rect width=\"2\" height=\"2\"/></svg>"
    )
    .into_bytes()
}

fn minimal_gif(comment: &str) -> Vec<u8> {
    let mut bytes = b"GIF89a".to_vec();
    bytes.extend_from_slice(&[2, 0, 2, 0, 0, 0, 0]);
    bytes.extend_from_slice(&[0x21, 0xFE, comment.len() as u8]);
    bytes.extend_from_slice(comment.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&[0x2C, 0, 0, 0, 0, 2, 0, 2, 0, 0]);
    bytes.extend_from_slice(&[2, 1, 0x44, 0, 0x3B]);
    bytes
}

fn png_crc(kind: &[u8; 4], data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for byte in kind.iter().chain(data.iter()) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut chunk = Vec::new();
    chunk.extend_from_slice(&(data.len() as u32).to_be_bytes());
    chunk.extend_from_slice(kind);
    chunk.extend_from_slice(data);
    chunk.extend_from_slice(&png_crc(kind, data).to_be_bytes());
    chunk
}

fn minimal_png(comment: &str) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1A\n".to_vec();
    bytes.extend_from_slice(&png_chunk(b"IHDR", &[0; 13]));
    bytes.extend_from_slice(&png_chunk(
        b"tEXt",
        format!("Comment\0{comment}").as_bytes(),
    ));
    bytes.extend_from_slice(&png_chunk(b"IEND", &[]));
    bytes
}

fn minimal_png_xmp(format: &str) -> Vec<u8> {
    let xmp = format!(
        "<x:xmpmeta><rdf:RDF><rdf:Description xmlns:dc=\"urn:dc\" dc:format=\"{format}\"/></rdf:RDF></x:xmpmeta>"
    );
    let mut itxt = b"XML:com.adobe.xmp\0\0\0\0\0".to_vec();
    itxt.extend_from_slice(xmp.as_bytes());
    let mut bytes = b"\x89PNG\r\n\x1A\n".to_vec();
    bytes.extend_from_slice(&png_chunk(b"IHDR", &[0; 13]));
    bytes.extend_from_slice(&png_chunk(b"iTXt", &itxt));
    bytes.extend_from_slice(&png_chunk(b"IDAT", &[1, 2, 3, 4]));
    bytes.extend_from_slice(&png_chunk(b"IEND", &[]));
    bytes
}

fn wav_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut chunk = kind.to_vec();
    chunk.extend_from_slice(&(data.len() as u32).to_le_bytes());
    chunk.extend_from_slice(data);
    if !data.len().is_multiple_of(2) {
        chunk.push(0);
    }
    chunk
}

fn minimal_wav(title: &str) -> Vec<u8> {
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&1_u16.to_le_bytes());
    fmt.extend_from_slice(&1_u16.to_le_bytes());
    fmt.extend_from_slice(&8_000_u32.to_le_bytes());
    fmt.extend_from_slice(&8_000_u32.to_le_bytes());
    fmt.extend_from_slice(&1_u16.to_le_bytes());
    fmt.extend_from_slice(&8_u16.to_le_bytes());
    let mut list = b"INFO".to_vec();
    list.extend_from_slice(&wav_chunk(b"INAM", format!("{title}\0").as_bytes()));
    let mut body = wav_chunk(b"fmt ", &fmt);
    body.extend_from_slice(&wav_chunk(b"LIST", &list));
    body.extend_from_slice(&wav_chunk(b"data", &[9, 8, 7, 6]));
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&((4 + body.len()) as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVE");
    bytes.extend_from_slice(&body);
    bytes
}

fn flac_block(last: bool, kind: u8, data: &[u8]) -> Vec<u8> {
    let length = u32::try_from(data.len()).expect("fixture block should fit");
    let mut block = vec![if last { 0x80 | kind } else { kind }];
    block.extend_from_slice(&length.to_be_bytes()[1..]);
    block.extend_from_slice(data);
    block
}

fn minimal_flac(title: &str) -> Vec<u8> {
    let mut streaminfo = vec![0_u8; 34];
    streaminfo[0..2].copy_from_slice(&4096_u16.to_be_bytes());
    streaminfo[2..4].copy_from_slice(&4096_u16.to_be_bytes());
    let packed = (44_100_u64 << 44) | (1_u64 << 41) | (15_u64 << 36) | 88_200;
    streaminfo[10..18].copy_from_slice(&packed.to_be_bytes());

    let vendor = b"Metra CLI";
    let comment = format!("TITLE={title}");
    let mut vorbis = (vendor.len() as u32).to_le_bytes().to_vec();
    vorbis.extend_from_slice(vendor);
    vorbis.extend_from_slice(&1_u32.to_le_bytes());
    vorbis.extend_from_slice(&(comment.len() as u32).to_le_bytes());
    vorbis.extend_from_slice(comment.as_bytes());

    let mut bytes = b"fLaC".to_vec();
    bytes.extend_from_slice(&flac_block(false, 0, &streaminfo));
    bytes.extend_from_slice(&flac_block(true, 4, &vorbis));
    bytes.extend_from_slice(&[1, 2, 3, 4]);
    bytes
}

fn id3_synchsafe(value: usize) -> [u8; 4] {
    [
        ((value >> 21) & 0x7F) as u8,
        ((value >> 14) & 0x7F) as u8,
        ((value >> 7) & 0x7F) as u8,
        (value & 0x7F) as u8,
    ]
}

fn id3_frame(id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut frame = id.to_vec();
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(&[0, 0]);
    frame.extend_from_slice(payload);
    frame
}

fn minimal_mp3(title: &str) -> Vec<u8> {
    let mut title_payload = vec![3];
    title_payload.extend_from_slice(title.as_bytes());
    let mut frames = id3_frame(b"TIT2", &title_payload);
    let mut comment = vec![3, b'e', b'n', b'g', 0];
    comment.extend_from_slice(b"source comment");
    frames.extend_from_slice(&id3_frame(b"COMM", &comment));
    frames.extend_from_slice(&[0; 8]);

    let mut bytes = b"ID3".to_vec();
    bytes.extend_from_slice(&[4, 0, 0]);
    bytes.extend_from_slice(&id3_synchsafe(frames.len()));
    bytes.extend_from_slice(&frames);
    bytes.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x64, 1, 2, 3, 4]);
    bytes
}

fn webp_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut chunk = kind.to_vec();
    chunk.extend_from_slice(&(data.len() as u32).to_le_bytes());
    chunk.extend_from_slice(data);
    if data.len() & 1 == 1 {
        chunk.push(0);
    }
    chunk
}

fn minimal_webp(format: &str) -> Vec<u8> {
    let xmp = format!(
        "<x:xmpmeta><rdf:RDF><rdf:Description xmlns:dc=\"urn:dc\" dc:format=\"{format}\"/></rdf:RDF></x:xmpmeta>"
    );
    let mut body = webp_chunk(b"VP8X", &[0, 0, 0, 0, 1, 0, 0, 1, 0, 0]);
    body.extend_from_slice(&webp_chunk(b"VP8 ", &[1, 2, 3]));
    body.extend_from_slice(&webp_chunk(b"XMP ", xmp.as_bytes()));
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&((4 + body.len()) as u32).to_le_bytes());
    bytes.extend_from_slice(b"WEBP");
    bytes.extend_from_slice(&body);
    bytes
}

fn run(args: &[&Path]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_metra"));
    for path in args {
        command.arg(path);
    }
    command.output().expect("Metra CLI should start")
}

#[test]
fn json_output_exposes_typed_exif_tags() {
    let directory = TemporaryDirectory::new();
    let path = directory.file("camera.jpg", &minimal_exif_jpeg());
    let output = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args(["--json", path.to_str().expect("UTF-8 test path")])
        .output()
        .expect("Metra CLI should start");

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let document: Value = serde_json::from_slice(&output.stdout).expect("JSON output should parse");
    assert_eq!(document["schema_version"], 1);
    assert_eq!(document["file_info"]["format"], "JPEG");
    let make = document["tags"]
        .as_array()
        .expect("tags should be an array")
        .iter()
        .find(|tag| tag["name"] == "Make")
        .expect("EXIF Make should be present");
    assert_eq!(make["namespace"], "EXIF");
    assert_eq!(make["value"]["string"], "Sony");
}

#[test]
fn jsonl_and_human_modes_are_available() {
    let directory = TemporaryDirectory::new();
    let path = directory.file("camera.jpg", &minimal_exif_jpeg());

    let jsonl = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args(["--jsonl", path.to_str().expect("UTF-8 test path")])
        .output()
        .expect("Metra CLI should start");
    assert!(jsonl.status.success());
    assert_eq!(
        jsonl
            .stdout
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .count(),
        1
    );

    let human = run(&[path.as_path()]);
    assert!(human.status.success());
    let stdout = String::from_utf8(human.stdout).expect("human output should be UTF-8");
    assert!(stdout.contains("Format: JPEG"));
    assert!(stdout.contains("EXIF:Make"));
}

#[test]
fn validate_returns_failure_for_recoverable_warnings() {
    let directory = TemporaryDirectory::new();
    let mut bytes = minimal_png("warning");
    let text_start = 8 + 25;
    let text_length = u32::from_be_bytes(
        bytes[text_start..text_start + 4]
            .try_into()
            .expect("PNG text length"),
    ) as usize;
    let crc_start = text_start + 8 + text_length;
    bytes[crc_start] ^= 0xFF;
    let path = directory.file("warning.png", &bytes);

    let regular = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([path.to_str().expect("UTF-8 test path")])
        .output()
        .expect("Metra CLI should start");
    assert!(regular.status.success(), "stderr: {:?}", regular.stderr);

    let strict = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--validate",
            "--json",
            path.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(!strict.status.success());
    let document: Value = serde_json::from_slice(&strict.stdout).expect("JSON output should parse");
    assert_eq!(document["warnings"][0]["code"], "png-crc");
}

#[test]
fn jobs_keep_batch_output_in_path_order() {
    let directory = TemporaryDirectory::new();
    let first = directory.file("camera-a.jpg", &minimal_exif_jpeg());
    let second = directory.file("camera-b.jpg", &minimal_exif_jpeg());
    let output = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--jsonl",
            "--jobs",
            "2",
            second.to_str().expect("UTF-8 test path"),
            first.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("JSONL output should be UTF-8");
    let mut lines = stdout.lines();
    assert!(
        lines
            .next()
            .is_some_and(|line| line.contains("camera-a.jpg"))
    );
    assert!(
        lines
            .next()
            .is_some_and(|line| line.contains("camera-b.jpg"))
    );
    assert!(lines.next().is_none());
}

#[test]
fn csv_output_contains_typed_tag_rows() {
    let directory = TemporaryDirectory::new();
    let path = directory.file("camera.csv.jpg", &minimal_exif_jpeg());
    let output = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args(["--csv", path.to_str().expect("UTF-8 test path")])
        .output()
        .expect("Metra CLI should start");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("CSV output should be UTF-8");
    assert!(
        stdout
            .lines()
            .next()
            .is_some_and(|line| { line == "path,format,namespace,group,id,name,value_type,value" })
    );
    assert!(stdout.contains("\"EXIF\""));
    assert!(stdout.contains("\"Make\""));
    assert!(stdout.contains("\"String\""));
}

#[test]
fn toml_and_yaml_outputs_keep_schema_version() {
    let directory = TemporaryDirectory::new();
    let path = directory.file("camera.jpg", &minimal_exif_jpeg());
    for format in ["--toml", "--yaml"] {
        let output = Command::new(env!("CARGO_BIN_EXE_metra"))
            .args([format, path.to_str().expect("UTF-8 test path")])
            .output()
            .expect("Metra CLI should start");
        assert!(output.status.success(), "stderr: {:?}", output.stderr);
        let text = String::from_utf8(output.stdout).expect("structured output should be UTF-8");
        assert!(text.contains("schema_version"));
        assert!(text.contains("Sony"));
    }
}

#[test]
fn svg_json_output_exposes_document_metadata() {
    let directory = TemporaryDirectory::new();
    let path = directory.file("drawing.svg", &minimal_svg());
    let output = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args(["--json", path.to_str().expect("UTF-8 test path")])
        .output()
        .expect("Metra CLI should start");

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let document: Value = serde_json::from_slice(&output.stdout).expect("JSON output should parse");
    assert_eq!(document["file_info"]["format"], "SVG");
    assert_eq!(document["file_info"]["mime_type"], "image/svg+xml");
    assert!(
        document["tags"]
            .as_array()
            .expect("tags should be an array")
            .iter()
            .any(|tag| tag["name"] == "Title" && tag["value"]["string"] == "CLI vector")
    );
}

#[test]
fn cli_can_set_and_delete_a_jpeg_comment_atomically() {
    let directory = TemporaryDirectory::new();
    let path = directory.file("editable.jpg", &minimal_exif_jpeg());
    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "JPEG:Comment=edited from cli",
            path.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    let after_set = metra::read(&path).expect("rewritten JPEG should remain readable");
    assert_eq!(
        after_set.find("JPEG:Comment").unwrap().display_value(),
        "edited from cli"
    );

    let delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "JPEG:Comment",
            path.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(delete.status.success(), "stderr: {:?}", delete.stderr);
    let after_delete = metra::read(&path).expect("rewritten JPEG should remain readable");
    assert!(after_delete.find("JPEG:Comment").is_none());
}

#[test]
fn cli_can_copy_a_jpeg_comment_between_files() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source.jpg", &minimal_exif_jpeg());
    let target = directory.file("target.jpg", &minimal_exif_jpeg());
    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "JPEG:Comment=copied value",
            source.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);

    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            &format!("JPEG:Comment={}", source.to_str().expect("UTF-8 test path")),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    let metadata = metra::read(&target).expect("target JPEG should remain readable");
    assert_eq!(
        metadata.find("JPEG:Comment").unwrap().display_value(),
        "copied value"
    );
}

#[test]
fn cli_can_edit_and_copy_jpeg_iptc() {
    let directory = TemporaryDirectory::new();
    let source = directory.file(
        "source-iptc.jpg",
        &minimal_iptc_jpeg(&["source keyword"], "source caption"),
    );
    let target = directory.file(
        "target-iptc.jpg",
        &minimal_iptc_jpeg(&["target keyword"], "target caption"),
    );

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "IPTC:Keywords=edited keyword",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("IPTC:Keywords")
            .unwrap()
            .display_value(),
        "edited keyword"
    );

    let delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "IPTC:Keywords",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(delete.status.success(), "stderr: {:?}", delete.stderr);
    assert!(
        metra::read(&target)
            .unwrap()
            .find("IPTC:Keywords")
            .is_none()
    );

    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            &format!(
                "IPTC:CaptionAbstract={}",
                source.to_str().expect("UTF-8 test path")
            ),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("IPTC:CaptionAbstract")
            .unwrap()
            .display_value(),
        "source caption"
    );
}

#[test]
fn cli_can_edit_and_copy_jpeg_xmp() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source-xmp.jpg", &minimal_jpeg_xmp("source"));
    let target = directory.file("target-xmp.jpg", &minimal_jpeg_xmp("target"));
    let replacement = r#"<x:xmpmeta><rdf:RDF><rdf:Description xmlns:dc="urn:dc" dc:format="edited"/></rdf:RDF></x:xmpmeta>"#;
    let set_assignment = format!("JPEG:XMP={replacement}");

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            set_assignment.as_str(),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("XMP:dc:format")
            .unwrap()
            .display_value(),
        "edited"
    );

    let delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "JPEG:XMP",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(delete.status.success(), "stderr: {:?}", delete.stderr);
    assert!(metra::read(&target).unwrap().find("XMP:Packet").is_none());

    let copy_assignment = format!("JPEG:XMP={}", source.to_str().expect("UTF-8 test path"));
    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            copy_assignment.as_str(),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("XMP:dc:format")
            .unwrap()
            .display_value(),
        "source"
    );
}

#[test]
fn cli_can_edit_and_copy_a_png_text_chunk() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source.png", &minimal_png("source value"));
    let target = directory.file("target.png", &minimal_png("target value"));

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "PNG:Text:Comment=edited value",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("PNG:Text:Comment")
            .unwrap()
            .display_value(),
        "edited value"
    );

    let delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "PNG:Text:Comment",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(delete.status.success(), "stderr: {:?}", delete.stderr);
    assert!(
        metra::read(&target)
            .unwrap()
            .find("PNG:Text:Comment")
            .is_none()
    );

    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            &format!(
                "PNG:Text:Comment={}",
                source.to_str().expect("UTF-8 test path")
            ),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("PNG:Text:Comment")
            .unwrap()
            .display_value(),
        "source value"
    );
}

#[test]
fn cli_can_edit_and_copy_png_xmp() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source-xmp.png", &minimal_png_xmp("source"));
    let target = directory.file("target-xmp.png", &minimal_png_xmp("target"));

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "PNG:XMP=<x:xmpmeta><rdf:RDF><rdf:Description xmlns:dc=\"urn:dc\" dc:format=\"edited\"/></rdf:RDF></x:xmpmeta>",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("XMP:dc:format")
            .unwrap()
            .display_value(),
        "edited"
    );

    let delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "PNG:XMP",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(delete.status.success(), "stderr: {:?}", delete.stderr);
    assert!(metra::read(&target).unwrap().find("XMP:Packet").is_none());

    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            &format!("PNG:XMP={}", source.to_str().expect("UTF-8 test path")),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("XMP:dc:format")
            .unwrap()
            .display_value(),
        "source"
    );
}

#[test]
fn cli_can_edit_and_copy_svg_document_text() {
    let directory = TemporaryDirectory::new();
    let source = directory.file(
        "source.svg",
        &minimal_svg_document("source title", "source description", "source comment"),
    );
    let target = directory.file(
        "target.svg",
        &minimal_svg_document("target title", "target description", "target comment"),
    );

    for (key, value, expected) in [
        ("SVG:Title", "edited title", "SVG:Title"),
        ("SVG:Description", "edited description", "SVG:Description"),
        ("SVG:Comment", "edited comment", "SVG:Comment"),
    ] {
        let assignment = format!("{key}={value}");
        let set = Command::new(env!("CARGO_BIN_EXE_metra"))
            .args([
                "--set",
                &assignment,
                target.to_str().expect("UTF-8 test path"),
            ])
            .output()
            .expect("Metra CLI should start");
        assert!(set.status.success(), "stderr: {:?}", set.stderr);
        assert_eq!(
            metra::read(&target)
                .unwrap()
                .find(expected)
                .unwrap()
                .display_value(),
            value
        );
    }

    let delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "SVG:Comment",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(delete.status.success(), "stderr: {:?}", delete.stderr);
    assert!(metra::read(&target).unwrap().find("SVG:Comment").is_none());

    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            &format!("SVG:Title={}", source.to_str().expect("UTF-8 test path")),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("SVG:Title")
            .unwrap()
            .display_value(),
        "source title"
    );
}

#[test]
fn cli_can_edit_and_copy_wav_info() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source.wav", &minimal_wav("source title"));
    let target = directory.file("target.wav", &minimal_wav("target title"));

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "WAV:Title=edited title",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("WAV:Title")
            .unwrap()
            .display_value(),
        "edited title"
    );

    let delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "WAV:Title",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(delete.status.success(), "stderr: {:?}", delete.stderr);
    assert!(metra::read(&target).unwrap().find("WAV:Title").is_none());

    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            &format!("WAV:Title={}", source.to_str().expect("UTF-8 test path")),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("WAV:Title")
            .unwrap()
            .display_value(),
        "source title"
    );
}

#[test]
fn cli_can_edit_and_copy_flac_comments() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source.flac", &minimal_flac("source title"));
    let target = directory.file("target.flac", &minimal_flac("target title"));

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "FLAC:Title=edited title",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("FLAC:Title")
            .unwrap()
            .display_value(),
        "edited title"
    );

    let delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "FLAC:Title",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(delete.status.success(), "stderr: {:?}", delete.stderr);
    assert!(metra::read(&target).unwrap().find("FLAC:Title").is_none());

    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            &format!("FLAC:Title={}", source.to_str().expect("UTF-8 test path")),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("FLAC:Title")
            .unwrap()
            .display_value(),
        "source title"
    );
}

#[test]
fn cli_can_edit_and_copy_id3_text() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source.mp3", &minimal_mp3("source title"));
    let target = directory.file("target.mp3", &minimal_mp3("target title"));

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "ID3:Title=edited title",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("ID3:Title")
            .unwrap()
            .display_value(),
        "edited title"
    );

    let set_comment = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "ID3:Comment=edited comment",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(
        set_comment.status.success(),
        "stderr: {:?}",
        set_comment.stderr
    );
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("ID3:Comment")
            .unwrap()
            .display_value(),
        "edited comment"
    );

    let delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "ID3:Comment",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(delete.status.success(), "stderr: {:?}", delete.stderr);
    assert!(metra::read(&target).unwrap().find("ID3:Comment").is_none());

    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            &format!("ID3:Title={}", source.to_str().expect("UTF-8 test path")),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("ID3:Title")
            .unwrap()
            .display_value(),
        "source title"
    );
}

#[test]
fn cli_can_edit_and_copy_gif_comments() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source.gif", &minimal_gif("source comment"));
    let target = directory.file("target.gif", &minimal_gif("target comment"));

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "GIF:Comment=edited comment",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("GIF:Comment")
            .unwrap()
            .display_value(),
        "edited comment"
    );

    let delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "GIF:Comment",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(delete.status.success(), "stderr: {:?}", delete.stderr);
    assert!(metra::read(&target).unwrap().find("GIF:Comment").is_none());

    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            &format!("GIF:Comment={}", source.to_str().expect("UTF-8 test path")),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("GIF:Comment")
            .unwrap()
            .display_value(),
        "source comment"
    );
}

#[test]
fn cli_can_edit_and_copy_webp_xmp() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source.webp", &minimal_webp("source"));
    let target = directory.file("target.webp", &minimal_webp("target"));

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "WebP:XMP=<x:xmpmeta><rdf:RDF><rdf:Description xmlns:dc=\"urn:dc\" dc:format=\"edited\"/></rdf:RDF></x:xmpmeta>",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("XMP:dc:format")
            .unwrap()
            .display_value(),
        "edited"
    );

    let delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "WebP:XMP",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(delete.status.success(), "stderr: {:?}", delete.stderr);
    assert!(metra::read(&target).unwrap().find("XMP:Packet").is_none());

    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            &format!("WebP:XMP={}", source.to_str().expect("UTF-8 test path")),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("XMP:dc:format")
            .unwrap()
            .display_value(),
        "source"
    );
}
