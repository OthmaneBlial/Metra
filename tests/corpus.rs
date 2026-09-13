use std::env;
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn corpus_files() -> Vec<PathBuf> {
    let root = env::var_os("METRA_CORPUS_DIR")
        .map(PathBuf::from)
        .expect("METRA_CORPUS_DIR must point to a reviewed local corpus");
    let mut files = Vec::new();
    collect_files(&root, &mut files).expect("corpus should be traversable");
    files.sort();
    assert!(
        !files.is_empty(),
        "the corpus directory should contain files"
    );
    files
}

fn collect_files(path: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_file() {
        files.push(path.to_path_buf());
        return Ok(());
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(path)? {
        collect_files(&entry?.path(), files)?;
    }
    Ok(())
}

#[test]
#[ignore = "requires METRA_CORPUS_DIR pointing to reviewed redistributable or private test media"]
fn corpus_inspection_does_not_panic() {
    let files = corpus_files();
    let mut recognized = 0_usize;
    let mut warnings = 0_usize;
    let mut failures = 0_usize;
    for path in files.iter() {
        let result = catch_unwind(AssertUnwindSafe(|| metra::read(path)));
        match result {
            Ok(Ok(metadata)) => {
                recognized += 1;
                warnings += metadata.warnings().len();
            }
            Ok(Err(error)) => {
                failures += 1;
                eprintln!("corpus: {}: {error}", path.display());
            }
            Err(_) => panic!("Metra panicked while inspecting {}", path.display()),
        }
    }
    eprintln!(
        "corpus summary: files={}, recognized={}, failures={}, warnings={warnings}",
        files.len(),
        recognized,
        failures
    );
}

#[test]
#[ignore = "requires METRA_CORPUS_DIR and METRA_ORACLE pointing to an ExifTool-compatible executable"]
fn corpus_supported_tags_can_be_compared_with_oracle() {
    let files = corpus_files();
    let oracle = env::var_os("METRA_ORACLE")
        .map(PathBuf::from)
        .expect("METRA_ORACLE must point to an ExifTool-compatible executable");
    let mut compared_files = 0_usize;
    let mut matched_tags = 0_usize;
    let mut metra_tags = 0_usize;

    for path in files {
        let Ok(metadata) = catch_unwind(AssertUnwindSafe(|| metra::read(&path))) else {
            panic!("Metra panicked while inspecting {}", path.display());
        };
        let Ok(metadata) = metadata else {
            continue;
        };
        let output = Command::new(&oracle)
            .args(["-j", "-G1", "-s", "-a", "-n", "--"])
            .arg(&path)
            .output()
            .unwrap_or_else(|error| panic!("cannot run oracle for {}: {error}", path.display()));
        assert!(
            output.status.success(),
            "oracle failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        let document: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "oracle returned invalid JSON for {}: {error}",
                path.display()
            )
        });
        let object = document
            .as_array()
            .and_then(|items| items.first())
            .and_then(Value::as_object)
            .expect("oracle JSON should contain one metadata object");
        compared_files += 1;
        metra_tags += metadata.tags().len();
        matched_tags += metadata
            .tags()
            .iter()
            .filter(|tag| object.contains_key(tag.key().as_str()))
            .count();
    }

    eprintln!(
        "differential summary: compared_files={compared_files}, metra_tags={metra_tags}, oracle_key_matches={matched_tags}"
    );
}
