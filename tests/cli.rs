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
