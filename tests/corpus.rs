use std::collections::{BTreeSet, HashSet};
use std::env;
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

#[derive(Debug, serde::Serialize)]
struct InspectionSummary {
    files: usize,
    recognized: usize,
    failures: usize,
    warnings: usize,
}

#[derive(Debug, Default, serde::Serialize)]
struct DifferentialSummary {
    corpus_files: usize,
    compared_files: usize,
    metra_read_failures: usize,
    metra_panics: usize,
    metra_tags: usize,
    oracle_keys: usize,
    oracle_only_keys: usize,
    oracle_key_matches: usize,
    oracle_key_misses: usize,
    oracle_value_matches: usize,
    oracle_value_mismatches: usize,
}

fn corpus_root() -> PathBuf {
    env::var_os("METRA_CORPUS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(checked_in_corpus_root)
}

fn checked_in_corpus_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/corpus")
}

fn corpus_files() -> Vec<PathBuf> {
    let root = corpus_root();
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
    write_json_report(
        "METRA_CORPUS_INSPECTION_REPORT",
        &InspectionSummary {
            files: files.len(),
            recognized,
            failures,
            warnings,
        },
    );
}

#[test]
fn checked_in_corpus_matches_manifest() {
    let manifest_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/CORPUS_MANIFEST.json");
    let manifest: Value = serde_json::from_slice(
        &fs::read(&manifest_path).expect("checked-in corpus manifest should be readable"),
    )
    .expect("checked-in corpus manifest should be valid JSON");
    assert_eq!(manifest["schema_version"], 1);

    let listed = manifest["files"]
        .as_array()
        .expect("corpus manifest files should be an array")
        .iter()
        .map(|entry| {
            let object = entry
                .as_object()
                .expect("corpus manifest entries should be objects");
            let name = object["name"]
                .as_str()
                .expect("corpus manifest entries should have names");
            let size = object["size_bytes"]
                .as_u64()
                .expect("corpus manifest entries should have sizes");
            let file = checked_in_corpus_root().join(name);
            assert!(
                file.is_file(),
                "manifest file should exist: {}",
                file.display()
            );
            assert_eq!(
                fs::metadata(&file)
                    .expect("manifest file metadata should be readable")
                    .len(),
                size,
                "manifest size drift for {name}"
            );
            name.to_owned()
        })
        .collect::<BTreeSet<_>>();

    let actual = corpus_files()
        .into_iter()
        .map(|file| {
            file.strip_prefix(checked_in_corpus_root())
                .expect("corpus file should be under corpus root")
                .to_string_lossy()
                .into_owned()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        listed, actual,
        "corpus files and manifest should stay in sync"
    );
}

#[test]
#[ignore = "requires METRA_CORPUS_DIR and METRA_ORACLE pointing to an ExifTool-compatible executable"]
fn corpus_supported_tags_can_be_compared_with_oracle() {
    let files = corpus_files();
    let oracle = env::var_os("METRA_ORACLE")
        .map(PathBuf::from)
        .expect("METRA_ORACLE must point to an ExifTool-compatible executable");
    let mut summary = DifferentialSummary {
        corpus_files: files.len(),
        ..DifferentialSummary::default()
    };

    for path in files {
        let Ok(metadata) = catch_unwind(AssertUnwindSafe(|| metra::read(&path))) else {
            summary.metra_panics += 1;
            eprintln!(
                "differential: Metra panicked while inspecting {}",
                path.display()
            );
            continue;
        };
        let Ok(metadata) = metadata else {
            summary.metra_read_failures += 1;
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
        summary.compared_files += 1;
        summary.metra_tags += metadata.tags().len();
        let mut matched_oracle_keys = HashSet::new();
        for tag in metadata.tags() {
            let Some(key) = oracle_key_candidates(tag)
                .into_iter()
                .find(|key| object.contains_key(key))
            else {
                summary.oracle_key_misses += 1;
                continue;
            };
            summary.oracle_key_matches += 1;
            matched_oracle_keys.insert(key.clone());
            if let Some(value) = object.get(&key) {
                if oracle_tag_matches(tag, value) {
                    summary.oracle_value_matches += 1;
                } else {
                    summary.oracle_value_mismatches += 1;
                    eprintln!(
                        "differential mismatch: {}: {} != {}",
                        path.display(),
                        tag.key(),
                        key
                    );
                }
            }
        }
        summary.oracle_keys += object.len();
        summary.oracle_only_keys += oracle_only_key_count(object, &matched_oracle_keys);
    }

    eprintln!(
        "differential summary: corpus_files={}, compared_files={}, metra_read_failures={}, metra_panics={}, metra_tags={}, oracle_keys={}, oracle_only_keys={}, oracle_key_matches={}, oracle_key_misses={}, oracle_value_matches={}, oracle_value_mismatches={}",
        summary.corpus_files,
        summary.compared_files,
        summary.metra_read_failures,
        summary.metra_panics,
        summary.metra_tags,
        summary.oracle_keys,
        summary.oracle_only_keys,
        summary.oracle_key_matches,
        summary.oracle_key_misses,
        summary.oracle_value_matches,
        summary.oracle_value_mismatches
    );
    write_json_report("METRA_CORPUS_DIFFERENTIAL_REPORT", &summary);

    if env_flag("METRA_ORACLE_STRICT") {
        assert_eq!(
            summary.metra_panics, 0,
            "strict differential mode found {} Metra panics",
            summary.metra_panics
        );
        assert_eq!(
            summary.oracle_key_misses, 0,
            "strict differential mode found {} Metra tags without an oracle key",
            summary.oracle_key_misses
        );
        assert_eq!(
            summary.oracle_value_mismatches, 0,
            "strict differential mode found {} mismatched values",
            summary.oracle_value_mismatches
        );
        if env_flag("METRA_ORACLE_REQUIRE_ALL") {
            assert_eq!(
                summary.oracle_only_keys, 0,
                "strict differential mode found {} oracle-only keys",
                summary.oracle_only_keys
            );
        }
    }
}

fn env_flag(name: &str) -> bool {
    env::var(name).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn write_json_report<T: serde::Serialize>(variable: &str, report: &T) {
    let Some(path) = env::var_os(variable).map(PathBuf::from) else {
        return;
    };
    let payload = serde_json::to_vec_pretty(report)
        .unwrap_or_else(|error| panic!("cannot serialize {variable}: {error}"));
    fs::write(&path, payload)
        .unwrap_or_else(|error| panic!("cannot write {variable} to {}: {error}", path.display()));
    eprintln!("corpus report written: {}", path.display());
}

fn oracle_only_key_count(
    object: &serde_json::Map<String, Value>,
    matched_oracle_keys: &HashSet<String>,
) -> usize {
    object
        .keys()
        .filter(|key| !matched_oracle_keys.contains(*key))
        .count()
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
        "Ogg" => match tag.name.as_str() {
            "Channels" => vec![
                "Vorbis:AudioChannels".to_owned(),
                "Opus:AudioChannels".to_owned(),
                "FLAC:Channels".to_owned(),
            ],
            "SampleRate" => vec![
                "Vorbis:SampleRate".to_owned(),
                "Opus:SampleRate".to_owned(),
                "FLAC:SampleRate".to_owned(),
            ],
            "BlockSizeMin" => vec!["FLAC:BlockSizeMin".to_owned()],
            "BlockSizeMax" => vec!["FLAC:BlockSizeMax".to_owned()],
            "FrameSizeMin" => vec!["FLAC:FrameSizeMin".to_owned()],
            "FrameSizeMax" => vec!["FLAC:FrameSizeMax".to_owned()],
            "BitsPerSample" => vec!["FLAC:BitsPerSample".to_owned()],
            "TotalSamples" => vec!["FLAC:TotalSamples".to_owned()],
            "MD5Signature" => vec!["FLAC:MD5Signature".to_owned()],
            "Vendor" => vec!["Vorbis:Vendor".to_owned()],
            "Encoder" => vec!["Vorbis:Encoder".to_owned()],
            "OutputGain" => vec!["Opus:OutputGain".to_owned()],
            name if name.starts_with("Comment:") => {
                let comment_name = &name["Comment:".len()..];
                match comment_name {
                    "COVERARTMIME" => vec!["Vorbis:CoverArtMIMEType".to_owned()],
                    "MEDIAJUKEBOX:DATE" => vec!["Vorbis:MediajukeboxDate".to_owned()],
                    "MEDIAJUKEBOX:TOOL NAME" => vec!["Vorbis:MediajukeboxToolName".to_owned()],
                    "MEDIAJUKEBOX:TOOL VERSION" => {
                        vec!["Vorbis:MediajukeboxToolVersion".to_owned()]
                    }
                    _ => vec![format!("Vorbis:{comment_name}")],
                }
            }
            name => vec![format!("Vorbis:{name}")],
        },
        "JPEG" if tag.group == "COM" => vec![format!("File:{name}")],
        "JFIF" => vec![format!("JFIF:{name}")],
        "ISOBMFF" => vec![format!("QuickTime:{name}"), format!("{name}")],
        "AVI" | "WAV" => vec![format!("RIFF:{name}"), format!("{}:{name}", tag.namespace)],
        _ => vec![format!("{}:{name}", tag.namespace)],
    }
}

fn oracle_value_matches(value: &metra::TagValue, oracle: &Value) -> bool {
    match value {
        metra::TagValue::String(value) => oracle.as_str() == Some(value),
        metra::TagValue::Unsigned(value) => oracle.as_u64() == Some(*value),
        metra::TagValue::Signed(value) => oracle.as_i64() == Some(*value),
        metra::TagValue::Float(value) => oracle_number_matches(*value, oracle),
        metra::TagValue::Date { .. }
        | metra::TagValue::Time { .. }
        | metra::TagValue::DateTime { .. } => oracle.as_str() == Some(&value.to_display_string()),
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

fn oracle_tag_matches(tag: &metra::Tag, oracle: &Value) -> bool {
    oracle_value_matches(&tag.value, oracle)
        || (tag.namespace == "PNG"
            && tag.group == "IHDR"
            && matches!(tag.value, metra::TagValue::String(_))
            && tag
                .raw_value
                .as_deref()
                .is_some_and(|raw| raw.len() == 1 && oracle.as_u64() == Some(u64::from(raw[0]))))
}

fn oracle_number_matches(value: f64, oracle: &Value) -> bool {
    oracle
        .as_f64()
        .is_some_and(|other| (value - other).abs() <= 1e-9 * value.abs().max(other.abs()).max(1.0))
}

fn rational_matches(numerator: f64, denominator: f64, oracle: &Value) -> bool {
    denominator != 0.0 && oracle_number_matches(numerator / denominator, oracle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oracle_candidates_preserve_group_aliases() {
        let tag = metra::Tag {
            namespace: "EXIF".into(),
            group: "IFD-next".into(),
            id: None,
            name: "ImageDescription".into(),
            description: None,
            value: metra::TagValue::String("example".into()),
            raw_value: None,
            value_type: metra::ValueType::String,
            source: metra::Source::default(),
            writable: false,
        };
        assert_eq!(oracle_key_candidates(&tag), ["IFD1:ImageDescription"]);
    }

    #[test]
    fn oracle_candidates_include_riff_alias_for_avi_and_wav_tags() {
        let tag = metra::Tag {
            namespace: "WAV".into(),
            group: "bext".into(),
            id: None,
            name: "DateTimeOriginal".into(),
            description: None,
            value: metra::TagValue::String("2026:09:14 12:34:56".into()),
            raw_value: None,
            value_type: metra::ValueType::String,
            source: metra::Source::default(),
            writable: false,
        };
        assert_eq!(
            oracle_key_candidates(&tag),
            ["RIFF:DateTimeOriginal", "WAV:DateTimeOriginal"]
        );
    }

    #[test]
    fn numeric_matching_allows_only_small_relative_drift() {
        assert!(oracle_number_matches(100.0, &Value::from(100.0000000001)));
        assert!(!oracle_number_matches(100.0, &Value::from(100.01)));
        assert!(rational_matches(1.0, 3.0, &Value::from(1.0 / 3.0)));
        assert!(!rational_matches(1.0, 0.0, &Value::from(0.0)));
    }

    #[test]
    fn arrays_require_same_shape_and_matching_values() {
        let value = metra::TagValue::Array(vec![
            metra::TagValue::Unsigned(1),
            metra::TagValue::Unsigned(2),
        ]);
        assert!(oracle_value_matches(&value, &serde_json::json!([1, 2])));
        assert!(!oracle_value_matches(&value, &serde_json::json!([1])));
        assert!(!oracle_value_matches(&value, &serde_json::json!([1, 3])));
    }

    #[test]
    fn png_enum_labels_can_match_their_raw_oracle_code() {
        let tag = metra::Tag {
            namespace: "PNG".into(),
            group: "IHDR".into(),
            id: None,
            name: "ColorType".into(),
            description: None,
            raw_value: Some(vec![6]),
            value: metra::TagValue::String("TrueColorAlpha".into()),
            value_type: metra::ValueType::String,
            source: metra::Source::default(),
            writable: false,
        };
        assert!(oracle_tag_matches(&tag, &serde_json::json!(6)));
        assert!(!oracle_tag_matches(&tag, &serde_json::json!(2)));
    }

    #[test]
    fn oracle_only_keys_are_counted_separately_from_metra_misses() {
        let oracle = serde_json::json!({
            "EXIF:Make": "Metra",
            "EXIF:Model": "Reference",
            "SourceFile": "fixture.tif"
        });
        let object = oracle
            .as_object()
            .expect("fixture oracle should be an object");
        let matched = HashSet::from(["EXIF:Make".to_owned()]);
        assert_eq!(object.len(), 3);
        assert_eq!(oracle_only_key_count(object, &matched), 2);
    }
}
