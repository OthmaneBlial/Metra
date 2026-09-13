use std::collections::BTreeMap;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use metra_core::{Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType};

#[derive(Debug, Default)]
struct Node {
    name: String,
    attributes: BTreeMap<String, String>,
    children: Vec<Node>,
    text: String,
}

pub(crate) fn parse_xmp(
    bytes: &[u8],
    data_offset: u64,
    container: &str,
    metadata: &mut Metadata,
    limits: ParseLimits,
) -> Result<()> {
    if bytes.len() > limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "XMP packet".to_owned(),
            limit: limits.max_value_bytes,
        });
    }
    let roots = parse_tree(bytes, limits)?;
    metadata.add_tag(Tag {
        namespace: "XMP".to_owned(),
        group: "Packet".to_owned(),
        id: None,
        name: "Packet".to_owned(),
        description: Some("Raw bounded XMP packet".to_owned()),
        raw_value: Some(bytes.to_vec()),
        value: TagValue::Bytes(bytes.to_vec()),
        value_type: ValueType::Bytes,
        source: Source::new(container, Some(data_offset), Some(bytes.len() as u64)),
        writable: false,
    });

    for root in &roots {
        emit_descriptions(root, data_offset, container, metadata);
    }
    Ok(())
}

fn parse_tree(bytes: &[u8], limits: ParseLimits) -> Result<Vec<Node>> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut stack: Vec<Node> = Vec::new();
    let mut roots = Vec::new();
    let mut node_count = 0_usize;
    let mut text_bytes = 0_usize;

    loop {
        let event =
            reader
                .read_event_into(&mut buffer)
                .map_err(|error| MetraError::InvalidXml {
                    message: error.to_string(),
                })?;
        match event {
            Event::Start(element) => {
                ensure_depth(stack.len(), limits)?;
                node_count = node_count.saturating_add(1);
                if node_count > limits.max_xmp_nodes {
                    return Err(MetraError::ResourceLimitExceeded {
                        resource: "XMP nodes".to_owned(),
                        limit: limits.max_xmp_nodes,
                    });
                }
                stack.push(node_from_start(&element, &reader)?);
            }
            Event::Empty(element) => {
                ensure_depth(stack.len(), limits)?;
                node_count = node_count.saturating_add(1);
                if node_count > limits.max_xmp_nodes {
                    return Err(MetraError::ResourceLimitExceeded {
                        resource: "XMP nodes".to_owned(),
                        limit: limits.max_xmp_nodes,
                    });
                }
                attach_node(node_from_start(&element, &reader)?, &mut stack, &mut roots);
            }
            Event::Text(text) => {
                let decoded = text.decode().map_err(|error| MetraError::InvalidXml {
                    message: error.to_string(),
                })?;
                let unescaped = quick_xml::escape::unescape(decoded.as_ref()).map_err(|error| {
                    MetraError::InvalidXml {
                        message: error.to_string(),
                    }
                })?;
                text_bytes = text_bytes.saturating_add(unescaped.len());
                if text_bytes > limits.max_value_bytes {
                    return Err(MetraError::ResourceLimitExceeded {
                        resource: "XMP text".to_owned(),
                        limit: limits.max_value_bytes,
                    });
                }
                if let Some(node) = stack.last_mut() {
                    node.text.push_str(unescaped.as_ref());
                }
            }
            Event::CData(text) => {
                let decoded = text.decode().map_err(|error| MetraError::InvalidXml {
                    message: error.to_string(),
                })?;
                text_bytes = text_bytes.saturating_add(decoded.len());
                if text_bytes > limits.max_value_bytes {
                    return Err(MetraError::ResourceLimitExceeded {
                        resource: "XMP text".to_owned(),
                        limit: limits.max_value_bytes,
                    });
                }
                if let Some(node) = stack.last_mut() {
                    node.text.push_str(decoded.as_ref());
                }
            }
            Event::End(end) => {
                let node = stack.pop().ok_or_else(|| MetraError::InvalidXml {
                    message: format!(
                        "unexpected closing element {}",
                        display_name(end.name().as_ref())
                    ),
                })?;
                attach_node(node, &mut stack, &mut roots);
            }
            Event::DocType(_) => {
                return Err(MetraError::InvalidXml {
                    message: "DOCTYPE is not allowed in metadata packets".to_owned(),
                });
            }
            Event::GeneralRef(reference) => {
                return Err(MetraError::InvalidXml {
                    message: format!(
                        "unresolved XML entity {} is not allowed in metadata packets",
                        display_name(reference.as_ref())
                    ),
                });
            }
            Event::Eof => break,
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) => {}
        }
        buffer.clear();
    }
    if !stack.is_empty() {
        return Err(MetraError::InvalidXml {
            message: "XML ended with unclosed elements".to_owned(),
        });
    }
    Ok(roots)
}

fn node_from_start(element: &BytesStart<'_>, reader: &Reader<&[u8]>) -> Result<Node> {
    let name = display_name(element.name().as_ref());
    let mut attributes = BTreeMap::new();
    for attribute in element.attributes().with_checks(true) {
        let attribute = attribute.map_err(|error| MetraError::InvalidXml {
            message: error.to_string(),
        })?;
        let key = display_name(attribute.key.as_ref());
        let value = attribute
            .decode_and_unescape_value(reader.decoder())
            .map_err(|error| MetraError::InvalidXml {
                message: error.to_string(),
            })?
            .into_owned();
        attributes.insert(key, value);
    }
    Ok(Node {
        name,
        attributes,
        children: Vec::new(),
        text: String::new(),
    })
}

fn attach_node(node: Node, stack: &mut [Node], roots: &mut Vec<Node>) {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(node);
    } else {
        roots.push(node);
    }
}

fn ensure_depth(depth: usize, limits: ParseLimits) -> Result<()> {
    if depth >= limits.max_recursion_depth {
        Err(MetraError::ResourceLimitExceeded {
            resource: "XMP nesting depth".to_owned(),
            limit: limits.max_recursion_depth,
        })
    } else {
        Ok(())
    }
}

fn emit_descriptions(node: &Node, data_offset: u64, container: &str, metadata: &mut Metadata) {
    if node.name == "rdf:Description" || node.name == "Description" {
        for (name, value) in &node.attributes {
            if is_metadata_control_attribute(name) {
                continue;
            }
            add_property(
                metadata,
                name,
                TagValue::String(value.clone()),
                data_offset,
                container,
            );
        }
        for child in &node.children {
            if child.name.starts_with("rdf:") {
                continue;
            }
            if let Some(value) = node_value(child) {
                add_property(metadata, &child.name, value, data_offset, container);
            }
        }
    }
    for child in &node.children {
        emit_descriptions(child, data_offset, container, metadata);
    }
}

fn is_metadata_control_attribute(name: &str) -> bool {
    name.starts_with("xmlns")
        || matches!(name, "rdf:about" | "rdf:nodeID" | "rdf:type" | "xml:lang")
}

fn node_value(node: &Node) -> Option<TagValue> {
    if let Some(container) = node
        .children
        .iter()
        .find(|child| matches!(child.name.as_str(), "rdf:Bag" | "rdf:Seq" | "rdf:Alt"))
    {
        let values = container
            .children
            .iter()
            .filter(|child| child.name == "rdf:li" || child.name == "li")
            .filter_map(node_value)
            .collect::<Vec<_>>();
        if container.name == "rdf:Alt" {
            let mut alternatives = BTreeMap::new();
            for (index, child) in container.children.iter().enumerate() {
                if let Some(value) = node_value(child) {
                    let language = child
                        .attributes
                        .get("xml:lang")
                        .cloned()
                        .unwrap_or_else(|| format!("item-{index}"));
                    alternatives.insert(language, value);
                }
            }
            return Some(TagValue::Structure(alternatives));
        }
        return Some(TagValue::Array(values));
    }

    if node.children.is_empty() {
        if let Some(resource) = node.attributes.get("rdf:resource") {
            return Some(TagValue::String(resource.clone()));
        }
        if !node.text.is_empty() {
            return Some(TagValue::String(node.text.clone()));
        }
        return if node.attributes.is_empty() {
            None
        } else {
            Some(TagValue::Structure(attributes_as_values(node)))
        };
    }

    let mut fields = BTreeMap::new();
    if !node.text.is_empty() {
        fields.insert("#text".to_owned(), TagValue::String(node.text.clone()));
    }
    for (name, value) in &node.attributes {
        if !is_metadata_control_attribute(name) {
            fields.insert(name.clone(), TagValue::String(value.clone()));
        }
    }
    for child in &node.children {
        if let Some(value) = node_value(child) {
            fields.insert(child.name.clone(), value);
        }
    }
    (!fields.is_empty()).then_some(TagValue::Structure(fields))
}

fn attributes_as_values(node: &Node) -> BTreeMap<String, TagValue> {
    node.attributes
        .iter()
        .filter(|(name, _)| !is_metadata_control_attribute(name))
        .map(|(name, value)| (name.clone(), TagValue::String(value.clone())))
        .collect()
}

fn add_property(
    metadata: &mut Metadata,
    name: &str,
    value: TagValue,
    data_offset: u64,
    container: &str,
) {
    metadata.add_tag(Tag {
        namespace: "XMP".to_owned(),
        group: "RDF/Description".to_owned(),
        id: None,
        name: name.to_owned(),
        description: Some("XMP property".to_owned()),
        raw_value: None,
        value_type: value_type(&value),
        value,
        source: Source::new(container, Some(data_offset), None),
        writable: false,
    });
}

fn value_type(value: &TagValue) -> ValueType {
    match value {
        TagValue::String(_) => ValueType::String,
        TagValue::Unsigned(_) => ValueType::UnsignedInteger,
        TagValue::Signed(_) => ValueType::SignedInteger,
        TagValue::Float(_) => ValueType::Float,
        TagValue::Rational { .. } => ValueType::Rational,
        TagValue::UnsignedRational { .. } => ValueType::UnsignedRational,
        TagValue::Bytes(_) | TagValue::Unknown { .. } => ValueType::Bytes,
        TagValue::Array(_) => ValueType::Array,
        TagValue::Structure(_) => ValueType::Structure,
    }
}

fn display_name(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use metra_core::{FileFormat, FileInfo};

    #[test]
    fn preserves_xmp_arrays_and_language_alternatives() {
        let packet = br#"<?xpacket begin=""?><x:xmpmeta><rdf:RDF><rdf:Description xmlns:dc="urn:dc" dc:format="image/jpeg"><dc:subject><rdf:Bag><rdf:li>rust</rdf:li><rdf:li>metadata</rdf:li></rdf:Bag></dc:subject><dc:title><rdf:Alt><rdf:li xml:lang="x-default">Metra</rdf:li><rdf:li xml:lang="fr-FR">Metra France</rdf:li></rdf:Alt></dc:title></rdf:Description></rdf:RDF></x:xmpmeta><?xpacket end="w"?>"#;
        let mut metadata = Metadata::new(FileInfo::new(
            "xmp.jpg".into(),
            packet.len() as u64,
            FileFormat::Jpeg,
        ));
        parse_xmp(
            packet,
            0,
            "JPEG/APP1-XMP",
            &mut metadata,
            ParseLimits::default(),
        )
        .expect("XMP fixture should parse");
        assert_eq!(
            metadata.find("XMP:dc:format").unwrap().display_value(),
            "image/jpeg"
        );
        assert!(matches!(
            metadata.find("XMP:dc:subject").unwrap().value,
            TagValue::Array(_)
        ));
        assert!(matches!(
            metadata.find("XMP:dc:title").unwrap().value,
            TagValue::Structure(_)
        ));
    }

    #[test]
    fn rejects_doctype_packets() {
        let mut metadata = Metadata::new(FileInfo::new("xmp.jpg".into(), 10, FileFormat::Jpeg));
        let result = parse_xmp(
            b"<!DOCTYPE x><x:xmpmeta/>",
            0,
            "JPEG/APP1-XMP",
            &mut metadata,
            ParseLimits::default(),
        );
        assert!(matches!(result, Err(MetraError::InvalidXml { .. })));
    }
}
