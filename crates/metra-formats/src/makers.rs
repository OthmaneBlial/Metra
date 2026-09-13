use metra_core::{Metadata, Source, Tag, TagValue, ValueType};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MakerNoteIdentity {
    vendor: &'static str,
    format: &'static str,
}

pub(crate) fn inspect_maker_note(bytes: &[u8], data_offset: u64, metadata: &mut Metadata) {
    let Some(identity) = identify(bytes) else {
        return;
    };
    add_tag(
        metadata,
        "Vendor",
        identity.vendor,
        data_offset,
        bytes.len() as u64,
    );
    add_tag(
        metadata,
        "Format",
        identity.format,
        data_offset,
        bytes.len() as u64,
    );
}

fn identify(bytes: &[u8]) -> Option<MakerNoteIdentity> {
    if bytes.starts_with(b"Nikon\0") {
        let format = match bytes.get(6..10) {
            Some([2, 0, 0, 0]) => "Nikon Type 2",
            Some([1, 0, 0, 0]) => "Nikon Type 1",
            _ => "Nikon MakerNote",
        };
        return Some(MakerNoteIdentity {
            vendor: "Nikon",
            format,
        });
    }
    if bytes.starts_with(b"Canon\0") {
        return Some(MakerNoteIdentity {
            vendor: "Canon",
            format: "Canon MakerNote",
        });
    }
    if bytes.starts_with(b"FUJIFILM") {
        return Some(MakerNoteIdentity {
            vendor: "Fujifilm",
            format: "Fujifilm MakerNote",
        });
    }
    if bytes.starts_with(b"SONY DSC ") {
        return Some(MakerNoteIdentity {
            vendor: "Sony",
            format: "Sony MakerNote",
        });
    }
    if bytes.starts_with(b"Panasonic") {
        return Some(MakerNoteIdentity {
            vendor: "Panasonic",
            format: "Panasonic MakerNote",
        });
    }
    if bytes.starts_with(b"OLYMP") {
        return Some(MakerNoteIdentity {
            vendor: "Olympus",
            format: "Olympus MakerNote",
        });
    }
    None
}

fn add_tag(metadata: &mut Metadata, name: &str, value: &str, offset: u64, length: u64) {
    metadata.add_tag(Tag {
        namespace: "MakerNotes".to_owned(),
        group: "Detection".to_owned(),
        id: None,
        name: name.to_owned(),
        description: Some("Detected MakerNote vendor or container format".to_owned()),
        raw_value: None,
        value: TagValue::String(value.to_owned()),
        value_type: ValueType::String,
        source: Source::new("EXIF/MakerNote", Some(offset), Some(length)),
        writable: false,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use metra_core::{FileFormat, FileInfo};

    #[test]
    fn identifies_known_maker_note_headers_without_decoding_proprietary_tags() {
        let mut metadata =
            Metadata::new(FileInfo::new("maker-note.jpg".into(), 16, FileFormat::Jpeg));
        inspect_maker_note(b"Nikon\0\x02\0\0\0opaque", 100, &mut metadata);
        assert_eq!(
            metadata.find("MakerNotes:Vendor").unwrap().display_value(),
            "Nikon"
        );
        assert_eq!(
            metadata.find("MakerNotes:Format").unwrap().display_value(),
            "Nikon Type 2"
        );
        assert_eq!(
            metadata.find("MakerNotes:Vendor").unwrap().source.offset,
            Some(100)
        );
    }

    #[test]
    fn ignores_unknown_maker_note_payloads() {
        let mut metadata =
            Metadata::new(FileInfo::new("maker-note.jpg".into(), 4, FileFormat::Jpeg));
        inspect_maker_note(b"opaque", 0, &mut metadata);
        assert!(metadata.tags.is_empty());
    }
}
