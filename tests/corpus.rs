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
    let mut matched_values = 0_usize;
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
        for tag in metadata.tags() {
            let Some(key) = oracle_key_candidates(tag)
                .into_iter()
                .find(|key| object.contains_key(key))
            else {
                continue;
            };
            matched_tags += 1;
            if let Some(value) = object.get(&key) {
                matched_values += usize::from(oracle_value_matches(&tag.value, value));
            }
        }
    }

    eprintln!(
        "differential summary: compared_files={compared_files}, metra_tags={metra_tags}, oracle_key_matches={matched_tags}, oracle_value_matches={matched_values}"
    );
}

fn oracle_key_candidates(tag: &metra::Tag) -> Vec<String> {
    let name = match (tag.namespace.as_str(), tag.name.as_str()) {
        ("IPTC", "Byline") => "By-line",
        ("IPTC", "BylineTitle") => "By-lineTitle",
        ("IPTC", "CaptionAbstract") => "Caption-Abstract",
        ("IPTC", "Country") => "Country-PrimaryLocationName",
        ("IPTC", "CountryCode") => "Country-PrimaryLocationCode",
        ("IPTC", "OriginalTransmissionReference") => "OriginalTransmissionReference",
        ("IPTC", "ProvinceState") => "Province-State",
        ("IPTC", "WriterEditor") => "Writer-Editor",
        ("ICC", "Copyright") => "ProfileCopyright",
        ("ICC", "Description") => "ProfileDescription",
        ("ICC", "CreationDate") => "ProfileDateTime",
        ("ICC", "DeviceClass") => "ProfileClass",
        ("ICC", "Manufacturer") => "DeviceManufacturer",
        ("ICC", "Platform") => "PrimaryPlatform",
        ("ICC", "Version") => "ProfileVersion",
        ("ICC", "ColorSpace") => "ColorSpaceData",
        ("ICC", "Illuminant") => "ConnectionSpaceIlluminant",
        _ => tag.name.as_str(),
    };

    match tag.namespace.as_str() {
        "EXIF" => {
            let group = match tag.group.as_str() {
                "IFD-next" => "IFD1",
                other => other,
            };
            vec![format!("{group}:{name}")]
        }
        "XMP" => tag
            .name
            .split_once(':')
            .map(|(prefix, property)| format!("XMP-{prefix}:{property}"))
            .into_iter()
            .collect(),
        "IPTC" => ["IPTC", "IPTC2", "IPTC3"]
            .into_iter()
            .map(|group| format!("{group}:{name}"))
            .collect(),
        "ICC" => {
            let group = if matches!(
                tag.name.as_str(),
                "ColorSpace"
                    | "CreationDate"
                    | "DeviceClass"
                    | "Illuminant"
                    | "Manufacturer"
                    | "Model"
                    | "PCS"
                    | "Platform"
                    | "ProfileSize"
                    | "RenderingIntent"
                    | "Version"
            ) {
                "ICC-header"
            } else {
                "ICC_Profile"
            };
            vec![format!("{group}:{name}")]
        }
        "JPEG" if tag.group == "COM" => vec![format!("File:{name}")],
        "JFIF" => vec![format!("JFIF:{name}")],
        "ISOBMFF" => vec![format!("QuickTime:{name}"), format!("{name}")],
        _ => vec![format!("{}:{name}", tag.namespace)],
    }
}

fn oracle_value_matches(value: &metra::TagValue, oracle: &Value) -> bool {
    match value {
        metra::TagValue::String(value) => oracle.as_str() == Some(value),
        metra::TagValue::Unsigned(value) => oracle.as_u64() == Some(*value),
        metra::TagValue::Signed(value) => oracle.as_i64() == Some(*value),
        metra::TagValue::Float(value) => oracle_number_matches(*value, oracle),
        metra::TagValue::Rational {
            numerator,
            denominator,
        } => rational_matches(*numerator as f64, *denominator as f64, oracle),
        metra::TagValue::UnsignedRational {
            numerator,
            denominator,
        } => rational_matches(*numerator as f64, *denominator as f64, oracle),
        metra::TagValue::Array(values) => oracle.as_array().is_some_and(|items| {
            items.len() == values.len()
                && values
                    .iter()
                    .zip(items)
                    .all(|(value, item)| oracle_value_matches(value, item))
        }),
        metra::TagValue::Bytes(_)
        | metra::TagValue::Structure(_)
        | metra::TagValue::Unknown { .. } => false,
    }
}

fn oracle_number_matches(value: f64, oracle: &Value) -> bool {
    oracle
        .as_f64()
        .is_some_and(|other| (value - other).abs() <= 1e-9 * value.abs().max(other.abs()).max(1.0))
}

fn rational_matches(numerator: f64, denominator: f64, oracle: &Value) -> bool {
    denominator != 0.0 && oracle_number_matches(numerator / denominator, oracle)
}
