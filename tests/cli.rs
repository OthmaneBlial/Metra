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

fn minimal_svg() -> Vec<u8> {
    br#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" width="32" height="24"><title>CLI vector</title></svg>"#
        .to_vec()
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
