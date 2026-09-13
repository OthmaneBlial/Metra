use std::cmp::Reverse;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result};
use quick_xml::Reader;
use quick_xml::events::Event;

use crate::atomic::atomic_replace;
use crate::svg::read_svg;
use crate::xml::resolve_general_ref;

/// Lossless SVG document-text edits that preserve unrelated source bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SvgEdit {
    SetTitle(String),
    DeleteTitles,
    SetDescription(String),
    DeleteDescriptions,
    SetComment(String),
    DeleteComments,
}

pub fn rewrite_svg<R: Read + Seek, W: Write>(
    reader: &mut R,
    writer: &mut W,
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[SvgEdit],
) -> Result<()> {
    read_svg(reader, file_info.clone(), limits)?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|source| write_io_error(&file_info.path, source))?;
    let length =
        usize::try_from(file_info.size).map_err(|_| MetraError::ResourceLimitExceeded {
            resource: "SVG document".to_owned(),
            limit: limits.max_metadata_bytes,
        })?;
    if length > limits.max_metadata_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "SVG document".to_owned(),
            limit: limits.max_metadata_bytes,
        });
    }
    let mut bytes = vec![0_u8; length];
    reader
        .read_exact(&mut bytes)
        .map_err(|source| write_io_error(&file_info.path, source))?;
    let output = rewrite_svg_bytes(&bytes, &file_info.path, limits, edits)?;
    write_all(writer, &output)
}

pub fn rewrite_svg_to_vec(
    bytes: &[u8],
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[SvgEdit],
) -> Result<Vec<u8>> {
    let mut reader = std::io::Cursor::new(bytes);
    let mut output = Vec::new();
    rewrite_svg(&mut reader, &mut output, file_info.clone(), limits, edits)?;
    let validation_info = FileInfo::new(file_info.path, output.len() as u64, FileFormat::Svg);
    read_svg(
        &mut std::io::Cursor::new(output.as_slice()),
        validation_info,
        limits,
    )?;
    Ok(output)
}

pub fn rewrite_svg_path(
    path: impl AsRef<Path>,
    limits: ParseLimits,
    edits: &[SvgEdit],
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    let source_metadata = fs::metadata(&path).map_err(|source| MetraError::Io {
        path: path.clone(),
        source,
    })?;
    let file_info = FileInfo::new(path.clone(), source_metadata.len(), FileFormat::Svg);
    let temp_path = temporary_path(&path)?;
    let result = (|| {
        let mut input = File::open(&path).map_err(|source| MetraError::Io {
            path: path.clone(),
            source,
        })?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|source| MetraError::WriteFailure {
                message: format!("cannot create {}: {source}", temp_path.display()),
            })?;
        rewrite_svg(&mut input, &mut output, file_info.clone(), limits, edits)?;
        output
            .sync_all()
            .map_err(|source| MetraError::WriteFailure {
                message: format!("cannot sync {}: {source}", temp_path.display()),
            })?;
        drop(output);

        let mut validation = File::open(&temp_path).map_err(|source| MetraError::Io {
            path: temp_path.clone(),
            source,
        })?;
        let written_size = validation
            .metadata()
            .map_err(|source| MetraError::Io {
                path: temp_path.clone(),
                source,
            })?
            .len();
        let written_info = FileInfo::new(temp_path.clone(), written_size, FileFormat::Svg);
        read_svg(&mut validation, written_info, limits)?;
        fs::set_permissions(&temp_path, source_metadata.permissions()).map_err(|source| {
            MetraError::WriteFailure {
                message: format!(
                    "cannot preserve permissions on {}: {source}",
                    temp_path.display()
                ),
            }
        })?;
        atomic_replace(&temp_path, &path).map_err(|source| MetraError::WriteFailure {
            message: format!("cannot atomically replace {}: {source}", path.display()),
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextKind {
    Title,
    Description,
    Comment,
}

#[derive(Debug)]
enum SvgAction {
    Set { kind: TextKind, value: String },
    Delete { kind: TextKind },
}

#[derive(Debug, Clone)]
struct OpenElement {
    qualified_name: String,
    local_name: String,
    start: usize,
    open_end: usize,
}

#[derive(Debug, Clone)]
struct ElementSpan {
    qualified_name: String,
    local_name: String,
    start: usize,
    open_end: usize,
    close_start: Option<usize>,
    end: usize,
}

#[derive(Debug, Clone, Copy)]
struct Span {
    start: usize,
    end: usize,
}

#[derive(Debug)]
struct DocumentSpans {
    root: ElementSpan,
    elements: Vec<ElementSpan>,
    comments: Vec<Span>,
}

#[derive(Debug)]
struct Replacement {
    start: usize,
    end: usize,
    bytes: Vec<u8>,
}

fn rewrite_svg_bytes(
    bytes: &[u8],
    path: &Path,
    limits: ParseLimits,
    edits: &[SvgEdit],
) -> Result<Vec<u8>> {
    let Some(action) = svg_action(edits, limits)? else {
        return Ok(bytes.to_vec());
    };
    let spans = collect_spans(bytes, limits)?;
    let replacements = build_replacements(bytes, &spans, &action)?;
    let output = apply_replacements(bytes, replacements)?;
    if output.len() > limits.max_metadata_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: format!("SVG output for {}", path.display()),
            limit: limits.max_metadata_bytes,
        });
    }
    Ok(output)
}

fn svg_action(edits: &[SvgEdit], limits: ParseLimits) -> Result<Option<SvgAction>> {
    let mut action = None;
    for edit in edits {
        action = Some(match edit {
            SvgEdit::SetTitle(value) => SvgAction::Set {
                kind: TextKind::Title,
                value: validate_text(value, "SVG title", limits)?,
            },
            SvgEdit::DeleteTitles => SvgAction::Delete {
                kind: TextKind::Title,
            },
            SvgEdit::SetDescription(value) => SvgAction::Set {
                kind: TextKind::Description,
                value: validate_text(value, "SVG description", limits)?,
            },
            SvgEdit::DeleteDescriptions => SvgAction::Delete {
                kind: TextKind::Description,
            },
            SvgEdit::SetComment(value) => {
                let value = validate_text(value, "SVG comment", limits)?;
                if value.contains("--") || value.ends_with('-') {
                    return Err(MetraError::WriteFailure {
                        message: "SVG comments cannot contain '--' or end with '-'".to_owned(),
                    });
                }
                SvgAction::Set {
                    kind: TextKind::Comment,
                    value,
                }
            }
            SvgEdit::DeleteComments => SvgAction::Delete {
                kind: TextKind::Comment,
            },
        });
    }
    Ok(action)
}

fn validate_text(value: &str, resource: &str, limits: ParseLimits) -> Result<String> {
    if value.len() > limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: resource.to_owned(),
            limit: limits.max_value_bytes,
        });
    }
    if !value.chars().all(valid_xml_char) {
        return Err(MetraError::WriteFailure {
            message: format!("{resource} contains characters forbidden by XML 1.0"),
        });
    }
    Ok(value.to_owned())
}

fn valid_xml_char(character: char) -> bool {
    matches!(character, '\u{9}' | '\u{A}' | '\u{D}')
        || ('\u{20}'..='\u{D7FF}').contains(&character)
        || ('\u{E000}'..='\u{FFFD}').contains(&character)
        || ('\u{10000}'..='\u{10FFFF}').contains(&character)
}

fn collect_spans(bytes: &[u8], limits: ParseLimits) -> Result<DocumentSpans> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut stack = Vec::<OpenElement>::new();
    let mut elements = Vec::new();
    let mut comments = Vec::new();
    let mut root = None;
    let mut seen_root = false;
    let mut closed_root = false;
    let mut node_count = 0_usize;

    loop {
        let event =
            reader
                .read_event_into(&mut buffer)
                .map_err(|error| MetraError::InvalidXml {
                    message: error.to_string(),
                })?;
        match event {
            Event::Start(element) => {
                count_node(&mut node_count, limits)?;
                if closed_root {
                    return Err(MetraError::InvalidXml {
                        message: "SVG contains content after its root element".to_owned(),
                    });
                }
                let position = event_position(&reader, "SVG start element position")?;
                let start = markup_start(bytes, position)?;
                let qualified_name = display_name(element.name().as_ref());
                let local_name = local_name(element.name().as_ref());
                if !seen_root {
                    if local_name != "svg" {
                        return Err(MetraError::InvalidXml {
                            message: "SVG document must start with an <svg> root".to_owned(),
                        });
                    }
                    seen_root = true;
                }
                stack.push(OpenElement {
                    qualified_name,
                    local_name,
                    start,
                    open_end: position,
                });
            }
            Event::Empty(element) => {
                count_node(&mut node_count, limits)?;
                if closed_root {
                    return Err(MetraError::InvalidXml {
                        message: "SVG contains content after its root element".to_owned(),
                    });
                }
                let position = event_position(&reader, "SVG empty element position")?;
                let start = markup_start(bytes, position)?;
                let qualified_name = display_name(element.name().as_ref());
                let local_name = local_name(element.name().as_ref());
                if !seen_root {
                    if local_name != "svg" {
                        return Err(MetraError::InvalidXml {
                            message: "SVG document must start with an <svg> root".to_owned(),
                        });
                    }
                    seen_root = true;
                }
                let span = ElementSpan {
                    qualified_name,
                    local_name: local_name.clone(),
                    start,
                    open_end: position,
                    close_start: None,
                    end: position,
                };
                if matches!(local_name.as_str(), "title" | "desc") {
                    elements.push(span.clone());
                }
                if local_name == "svg" && stack.is_empty() {
                    root = Some(span);
                    closed_root = true;
                }
            }
            Event::End(element) => {
                let position = event_position(&reader, "SVG end element position")?;
                let close_start = markup_start(bytes, position)?;
                let open = stack.pop().ok_or_else(|| MetraError::InvalidXml {
                    message: "SVG contains an unexpected closing element".to_owned(),
                })?;
                let closing_name = local_name(element.name().as_ref());
                if open.local_name != closing_name {
                    return Err(MetraError::InvalidXml {
                        message: format!(
                            "closing element </{closing_name}> does not match <{}>",
                            open.local_name
                        ),
                    });
                }
                let span = ElementSpan {
                    qualified_name: open.qualified_name,
                    local_name: open.local_name,
                    start: open.start,
                    open_end: open.open_end,
                    close_start: Some(close_start),
                    end: position,
                };
                if matches!(span.local_name.as_str(), "title" | "desc") {
                    elements.push(span.clone());
                }
                if span.local_name == "svg" && stack.is_empty() {
                    root = Some(span);
                    closed_root = true;
                }
            }
            Event::Comment(_) => {
                let position = event_position(&reader, "SVG comment position")?;
                let start = markup_start(bytes, position)?;
                comments.push(Span {
                    start,
                    end: position,
                });
            }
            Event::DocType(_) => {
                return Err(MetraError::InvalidXml {
                    message: "DOCTYPE is not allowed in SVG metadata".to_owned(),
                });
            }
            Event::GeneralRef(reference) => {
                resolve_general_ref(&reference)?;
            }
            Event::Eof => break,
            Event::Text(_) | Event::CData(_) | Event::Decl(_) | Event::PI(_) => {}
        }
        buffer.clear();
    }

    if !seen_root || !stack.is_empty() {
        return Err(MetraError::InvalidXml {
            message: "SVG document has no complete root element".to_owned(),
        });
    }
    Ok(DocumentSpans {
        root: root.ok_or_else(|| MetraError::InvalidXml {
            message: "SVG document has no root element".to_owned(),
        })?,
        elements,
        comments,
    })
}

fn count_node(node_count: &mut usize, limits: ParseLimits) -> Result<()> {
    *node_count = node_count.saturating_add(1);
    if *node_count > limits.max_xmp_nodes {
        Err(MetraError::ResourceLimitExceeded {
            resource: "SVG XML nodes".to_owned(),
            limit: limits.max_xmp_nodes,
        })
    } else {
        Ok(())
    }
}

fn event_position(reader: &Reader<&[u8]>, resource: &str) -> Result<usize> {
    usize::try_from(reader.buffer_position()).map_err(|_| MetraError::ResourceLimitExceeded {
        resource: resource.to_owned(),
        limit: usize::MAX,
    })
}

fn markup_start(bytes: &[u8], end: usize) -> Result<usize> {
    bytes
        .get(..end)
        .and_then(|prefix| prefix.iter().rposition(|byte| *byte == b'<'))
        .ok_or_else(|| MetraError::InvalidXml {
            message: "SVG event has no source markup".to_owned(),
        })
}

fn build_replacements(
    bytes: &[u8],
    spans: &DocumentSpans,
    action: &SvgAction,
) -> Result<Vec<Replacement>> {
    match action {
        SvgAction::Set { kind, value } => {
            let escaped = escape_xml(value);
            let target = match kind {
                TextKind::Title => spans
                    .elements
                    .iter()
                    .find(|element| element.local_name == "title"),
                TextKind::Description => spans
                    .elements
                    .iter()
                    .find(|element| element.local_name == "desc"),
                TextKind::Comment => None,
            };
            if let Some(element) = target {
                if let Some(close_start) = element.close_start {
                    return Ok(vec![Replacement {
                        start: element.open_end,
                        end: close_start,
                        bytes: escaped.into_bytes(),
                    }]);
                }
                return Ok(vec![Replacement {
                    start: element.start,
                    end: element.end,
                    bytes: element_with_text(&element.qualified_name, &escaped),
                }]);
            }
            if *kind == TextKind::Comment {
                if let Some(comment) = spans.comments.first() {
                    return Ok(vec![Replacement {
                        start: comment.start,
                        end: comment.end,
                        bytes: format!("<!--{value}-->").into_bytes(),
                    }]);
                }
                return insertion(bytes, &spans.root, &format!("<!--{value}-->"));
            }
            let name = match kind {
                TextKind::Title => "title",
                TextKind::Description => "desc",
                TextKind::Comment => unreachable!(),
            };
            insertion(bytes, &spans.root, &format!("<{name}>{escaped}</{name}>"))
        }
        SvgAction::Delete { kind } => {
            if *kind == TextKind::Comment {
                return Ok(spans
                    .comments
                    .iter()
                    .map(|comment| Replacement {
                        start: comment.start,
                        end: comment.end,
                        bytes: Vec::new(),
                    })
                    .collect());
            }
            let local_name = match kind {
                TextKind::Title => "title",
                TextKind::Description => "desc",
                TextKind::Comment => unreachable!(),
            };
            Ok(spans
                .elements
                .iter()
                .filter(|element| element.local_name == local_name)
                .map(|element| Replacement {
                    start: element.start,
                    end: element.end,
                    bytes: Vec::new(),
                })
                .collect())
        }
    }
}

fn element_with_text(name: &str, escaped: &str) -> Vec<u8> {
    format!("<{name}>{escaped}</{name}>").into_bytes()
}

fn insertion(bytes: &[u8], root: &ElementSpan, fragment: &str) -> Result<Vec<Replacement>> {
    if let Some(close_start) = root.close_start {
        return Ok(vec![Replacement {
            start: close_start,
            end: close_start,
            bytes: fragment.as_bytes().to_vec(),
        }]);
    }
    let opening = bytes
        .get(root.start..root.end)
        .ok_or_else(|| MetraError::InvalidXml {
            message: "SVG root span is outside the document".to_owned(),
        })?;
    let close = opening
        .iter()
        .rposition(|byte| *byte == b'>')
        .ok_or_else(|| MetraError::InvalidXml {
            message: "SVG root element has no closing delimiter".to_owned(),
        })?;
    let mut slash = close;
    while slash > 0 && opening[slash - 1].is_ascii_whitespace() {
        slash -= 1;
    }
    if slash == 0 || opening[slash - 1] != b'/' {
        return Err(MetraError::InvalidXml {
            message: "SVG self-closing root has no slash".to_owned(),
        });
    }
    let mut replacement = opening[..slash - 1].to_vec();
    replacement.push(b'>');
    replacement.extend_from_slice(fragment.as_bytes());
    replacement.extend_from_slice(format!("</{}>", root.qualified_name).as_bytes());
    Ok(vec![Replacement {
        start: root.start,
        end: root.end,
        bytes: replacement,
    }])
}

fn apply_replacements(bytes: &[u8], mut replacements: Vec<Replacement>) -> Result<Vec<u8>> {
    replacements.sort_by_key(|replacement| (replacement.start, Reverse(replacement.end)));
    let mut output = Vec::with_capacity(bytes.len());
    let mut cursor = 0_usize;
    for replacement in replacements {
        if replacement.start < cursor || replacement.end < replacement.start {
            return Err(MetraError::WriteFailure {
                message: "SVG metadata edit spans overlap".to_owned(),
            });
        }
        let Some(prefix) = bytes.get(cursor..replacement.start) else {
            return Err(MetraError::WriteFailure {
                message: "SVG metadata edit span is outside the document".to_owned(),
            });
        };
        output.extend_from_slice(prefix);
        output.extend_from_slice(&replacement.bytes);
        cursor = replacement.end;
    }
    let Some(suffix) = bytes.get(cursor..) else {
        return Err(MetraError::WriteFailure {
            message: "SVG metadata edit end is outside the document".to_owned(),
        });
    };
    output.extend_from_slice(suffix);
    Ok(output)
}

fn escape_xml(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            character => escaped.push(character),
        }
    }
    escaped
}

fn local_name(bytes: &[u8]) -> String {
    let name = display_name(bytes);
    name.rsplit(':').next().unwrap_or(&name).to_owned()
}

fn display_name(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn write_all<W: Write>(writer: &mut W, bytes: &[u8]) -> Result<()> {
    writer
        .write_all(bytes)
        .map_err(|source| MetraError::WriteFailure {
            message: source.to_string(),
        })
}

fn write_io_error(path: &Path, source: std::io::Error) -> MetraError {
    if source.kind() == std::io::ErrorKind::UnexpectedEof {
        MetraError::UnexpectedEof {
            context: path.display().to_string(),
        }
    } else {
        MetraError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("metadata.svg");
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| MetraError::WriteFailure {
            message: format!("cannot create temporary name: {error}"),
        })?
        .as_nanos();
    Ok(path.with_file_name(format!(
        ".{filename}.metra-{}-{timestamp}.tmp",
        std::process::id()
    )))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    const SOURCE: &[u8] = br#"<?xml version="1.0"?>
<!-- original comment -->
<svg xmlns="http://www.w3.org/2000/svg" width="32" height="24">
  <title>Before</title>
  <desc>Old description</desc>
  <rect x="1" y="2" width="3" height="4"/>
</svg>"#;

    fn info(size: usize) -> FileInfo {
        FileInfo::new("drawing.svg".into(), size as u64, FileFormat::Svg)
    }

    #[test]
    fn replaces_text_and_preserves_unrelated_svg_source() {
        let output = rewrite_svg_to_vec(
            SOURCE,
            info(SOURCE.len()),
            ParseLimits::default(),
            &[SvgEdit::SetTitle("After & review".to_owned())],
        )
        .unwrap();
        let text = String::from_utf8(output.clone()).unwrap();
        assert!(text.contains("<title>After &amp; review</title>"));
        assert!(text.contains("<rect x=\"1\" y=\"2\" width=\"3\" height=\"4\"/>"));
        let metadata = read_svg(
            &mut Cursor::new(output.as_slice()),
            info(output.len()),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("SVG:Title").unwrap().display_value(),
            "After & review"
        );
    }

    #[test]
    fn deletes_and_inserts_document_text() {
        let deleted = rewrite_svg_to_vec(
            SOURCE,
            info(SOURCE.len()),
            ParseLimits::default(),
            &[SvgEdit::DeleteDescriptions],
        )
        .unwrap();
        let deleted_metadata = read_svg(
            &mut Cursor::new(deleted.as_slice()),
            info(deleted.len()),
            ParseLimits::default(),
        )
        .unwrap();
        assert!(deleted_metadata.find("SVG:Description").is_none());

        let without_title = br#"<svg xmlns="http://www.w3.org/2000/svg"><rect/></svg>"#;
        let inserted = rewrite_svg_to_vec(
            without_title,
            info(without_title.len()),
            ParseLimits::default(),
            &[SvgEdit::SetTitle("Inserted".to_owned())],
        )
        .unwrap();
        let inserted_metadata = read_svg(
            &mut Cursor::new(inserted.as_slice()),
            info(inserted.len()),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            inserted_metadata.find("SVG:Title").unwrap().display_value(),
            "Inserted"
        );
    }

    #[test]
    fn edits_comments_and_self_closing_roots() {
        let output = rewrite_svg_to_vec(
            SOURCE,
            info(SOURCE.len()),
            ParseLimits::default(),
            &[SvgEdit::SetComment("safe comment".to_owned())],
        )
        .unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("<!--safe comment-->"));
        assert!(!text.contains("<!-- original comment -->"));

        let empty = br#"<svg xmlns="http://www.w3.org/2000/svg"/>"#;
        let expanded = rewrite_svg_to_vec(
            empty,
            info(empty.len()),
            ParseLimits::default(),
            &[SvgEdit::SetDescription("Expanded".to_owned())],
        )
        .unwrap();
        let metadata = read_svg(
            &mut Cursor::new(expanded.as_slice()),
            info(expanded.len()),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("SVG:Description").unwrap().display_value(),
            "Expanded"
        );
    }

    #[test]
    fn rejects_invalid_comment_values() {
        let error = rewrite_svg_to_vec(
            SOURCE,
            info(SOURCE.len()),
            ParseLimits::default(),
            &[SvgEdit::SetComment("not--safe".to_owned())],
        )
        .unwrap_err();
        assert!(error.to_string().contains("comments"));
    }
}
