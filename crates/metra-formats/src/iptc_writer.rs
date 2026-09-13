use metra_core::{MetraError, ParseLimits, Result};

const PHOTOSHOP_PREFIX: &[u8] = b"Photoshop 3.0\0";
const IPTC_RESOURCE_ID: u16 = 0x0404;

#[derive(Debug)]
pub(crate) enum IptcAction {
    Set { dataset: u8, value: Vec<u8> },
    Delete { dataset: u8 },
}

pub(crate) fn set_action(name: &str, value: &str, limits: ParseLimits) -> Result<IptcAction> {
    let dataset = dataset_number(name)?;
    validate_value(value, limits)?;
    Ok(IptcAction::Set {
        dataset,
        value: value.as_bytes().to_vec(),
    })
}

pub(crate) fn delete_action(name: &str) -> Result<IptcAction> {
    Ok(IptcAction::Delete {
        dataset: dataset_number(name)?,
    })
}

pub(crate) fn rewrite_photoshop_app13(
    data: &[u8],
    action: &IptcAction,
    limits: ParseLimits,
) -> Result<Option<Vec<u8>>> {
    if !data.starts_with(PHOTOSHOP_PREFIX) {
        return Ok(None);
    }
    if data.len() > limits.max_metadata_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "JPEG Photoshop resources".to_owned(),
            limit: limits.max_metadata_bytes,
        });
    }

    let mut output = Vec::with_capacity(data.len());
    output.extend_from_slice(PHOTOSHOP_PREFIX);
    let mut cursor = PHOTOSHOP_PREFIX.len();
    let mut found_iptc_resource = false;
    while cursor < data.len() {
        let resource = parse_resource(data, cursor)?;
        if resource.id == IPTC_RESOURCE_ID && !found_iptc_resource {
            let updated = rewrite_iim(
                &data[resource.value_start..resource.value_end],
                action,
                limits,
            )?;
            output.extend_from_slice(&data[cursor..resource.size_start]);
            output.extend_from_slice(&(updated.len() as u32).to_be_bytes());
            output.extend_from_slice(&updated);
            if updated.len() & 1 == 1 {
                output.push(0);
            }
            found_iptc_resource = true;
        } else {
            output.extend_from_slice(&data[cursor..resource.end]);
        }
        cursor = resource.end;
    }

    if !found_iptc_resource && matches!(action, IptcAction::Set { .. }) {
        output.extend_from_slice(&new_resource(action)?);
    }
    ensure_output_budget(&output, limits)
}

pub(crate) fn new_photoshop_app13(action: &IptcAction, limits: ParseLimits) -> Result<Vec<u8>> {
    let mut data = PHOTOSHOP_PREFIX.to_vec();
    data.extend_from_slice(&new_resource(action)?);
    ensure_output_budget(&data, limits)?;
    let segment_length = data
        .len()
        .checked_add(2)
        .ok_or_else(|| MetraError::WriteFailure {
            message: "JPEG APP13 length overflowed".to_owned(),
        })?;
    if u16::try_from(segment_length).is_err() {
        return Err(MetraError::WriteFailure {
            message: "JPEG IPTC APP13 exceeds the 65533-byte segment limit".to_owned(),
        });
    }
    let mut segment = vec![0xFF, 0xED];
    segment.extend_from_slice(&(segment_length as u16).to_be_bytes());
    segment.extend_from_slice(&data);
    Ok(segment)
}

#[derive(Debug)]
struct ResourceSpan {
    id: u16,
    size_start: usize,
    value_start: usize,
    value_end: usize,
    end: usize,
}

fn parse_resource(data: &[u8], cursor: usize) -> Result<ResourceSpan> {
    if data.len().saturating_sub(cursor) < 8 {
        return Err(MetraError::InvalidTag {
            context: "JPEG Photoshop resource".to_owned(),
            message: "resource header is truncated".to_owned(),
        });
    }
    if &data[cursor..cursor + 4] != b"8BIM" {
        return Err(MetraError::InvalidTag {
            context: "JPEG Photoshop resource".to_owned(),
            message: "expected 8BIM signature".to_owned(),
        });
    }
    let id = u16::from_be_bytes([data[cursor + 4], data[cursor + 5]]);
    let name_length = usize::from(data[cursor + 6]);
    let name_start = cursor + 7;
    let name_end = name_start
        .checked_add(name_length)
        .ok_or_else(|| invalid_resource("resource name overflowed"))?;
    let size_start = name_end
        .checked_add((name_length + 1) % 2)
        .ok_or_else(|| invalid_resource("resource name padding overflowed"))?;
    let size_end = size_start
        .checked_add(4)
        .ok_or_else(|| invalid_resource("resource size overflowed"))?;
    if size_end > data.len() {
        return Err(invalid_resource("resource size is truncated"));
    }
    let value_length = usize::try_from(u32::from_be_bytes(
        data[size_start..size_end]
            .try_into()
            .expect("resource size is four bytes"),
    ))
    .map_err(|_| invalid_resource("resource length does not fit usize"))?;
    let value_start = size_end;
    let value_end = value_start
        .checked_add(value_length)
        .ok_or_else(|| invalid_resource("resource value overflowed"))?;
    let end = value_end
        .checked_add(value_length % 2)
        .ok_or_else(|| invalid_resource("resource padding overflowed"))?;
    if name_end > data.len() || end > data.len() {
        return Err(invalid_resource("resource extends beyond APP13"));
    }
    Ok(ResourceSpan {
        id,
        size_start,
        value_start,
        value_end,
        end,
    })
}

fn rewrite_iim(data: &[u8], action: &IptcAction, limits: ParseLimits) -> Result<Vec<u8>> {
    if data.len() > limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "IPTC resource".to_owned(),
            limit: limits.max_value_bytes,
        });
    }
    let target = action_dataset(action);
    let mut output = Vec::with_capacity(data.len());
    let mut cursor = 0_usize;
    while cursor < data.len() {
        let dataset_start = cursor;
        if data.len().saturating_sub(cursor) < 5 || data[cursor] != 0x1C {
            return Err(MetraError::InvalidTag {
                context: "IPTC IIM dataset".to_owned(),
                message: "dataset header is invalid".to_owned(),
            });
        }
        let record = data[cursor + 1];
        let dataset = data[cursor + 2];
        let length_marker = u16::from_be_bytes([data[cursor + 3], data[cursor + 4]]);
        cursor += 5;
        let value_length = if length_marker & 0x8000 == 0 {
            usize::from(length_marker)
        } else {
            let byte_count = usize::from(length_marker & 0x7FFF);
            if byte_count == 0 || byte_count > 8 || data.len().saturating_sub(cursor) < byte_count {
                return Err(MetraError::InvalidTag {
                    context: "IPTC IIM dataset".to_owned(),
                    message: "extended dataset length is invalid".to_owned(),
                });
            }
            let mut length = 0_usize;
            for byte in &data[cursor..cursor + byte_count] {
                length = length
                    .checked_mul(256)
                    .and_then(|value| value.checked_add(usize::from(*byte)))
                    .ok_or_else(|| invalid_resource("IPTC dataset length overflowed"))?;
            }
            cursor += byte_count;
            length
        };
        let value_end = cursor
            .checked_add(value_length)
            .ok_or_else(|| invalid_resource("IPTC dataset value overflowed"))?;
        if value_end > data.len() {
            return Err(MetraError::InvalidTag {
                context: "IPTC IIM dataset".to_owned(),
                message: "dataset value extends beyond the resource".to_owned(),
            });
        }
        if record != 2 || dataset != target {
            output.extend_from_slice(&data[dataset_start..value_end]);
        }
        cursor = value_end;
    }

    if let IptcAction::Set { dataset, value } = action {
        output.push(0x1C);
        output.push(2);
        output.push(*dataset);
        output.extend_from_slice(&(value.len() as u16).to_be_bytes());
        output.extend_from_slice(value);
    }
    ensure_output_budget(&output, limits)?;
    Ok(output)
}

fn new_resource(action: &IptcAction) -> Result<Vec<u8>> {
    let IptcAction::Set { dataset, value } = action else {
        return Err(MetraError::WriteFailure {
            message: "cannot create an IPTC resource for a delete action".to_owned(),
        });
    };
    let mut resource = vec![0x1C, 2, *dataset];
    resource.extend_from_slice(&(value.len() as u16).to_be_bytes());
    resource.extend_from_slice(value);

    let mut block = b"8BIM".to_vec();
    block.extend_from_slice(&IPTC_RESOURCE_ID.to_be_bytes());
    block.extend_from_slice(&[0, 0]);
    block.extend_from_slice(&(resource.len() as u32).to_be_bytes());
    block.extend_from_slice(&resource);
    if resource.len() & 1 == 1 {
        block.push(0);
    }
    Ok(block)
}

fn action_dataset(action: &IptcAction) -> u8 {
    match action {
        IptcAction::Set { dataset, .. } | IptcAction::Delete { dataset } => *dataset,
    }
}

fn dataset_number(name: &str) -> Result<u8> {
    let dataset = match name {
        "ObjectName" => 3,
        "EditStatus" => 5,
        "Urgency" => 10,
        "Category" => 15,
        "SupplementalCategories" => 20,
        "Keywords" => 25,
        "DateCreated" => 55,
        "TimeCreated" => 60,
        "Byline" => 80,
        "BylineTitle" => 85,
        "City" => 90,
        "SubLocation" => 92,
        "ProvinceState" => 95,
        "CountryCode" => 101,
        "Country" => 102,
        "Headline" => 105,
        "Credit" => 110,
        "Source" => 115,
        "CopyrightNotice" => 116,
        "CaptionAbstract" => 120,
        "WriterEditor" => 122,
        _ => {
            return Err(MetraError::WriteFailure {
                message: format!("unsupported IPTC dataset {name}"),
            });
        }
    };
    Ok(dataset)
}

fn validate_value(value: &str, limits: ParseLimits) -> Result<()> {
    if value.contains('\0') {
        return Err(MetraError::WriteFailure {
            message: "IPTC text values cannot contain NUL".to_owned(),
        });
    }
    if value.len() > limits.max_value_bytes || value.len() > usize::from(u16::MAX) {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "IPTC dataset value".to_owned(),
            limit: limits.max_value_bytes.min(usize::from(u16::MAX)),
        });
    }
    Ok(())
}

fn ensure_output_budget(output: &[u8], limits: ParseLimits) -> Result<Option<Vec<u8>>> {
    if output.len() > limits.max_metadata_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "JPEG Photoshop rewrite".to_owned(),
            limit: limits.max_metadata_bytes,
        });
    }
    Ok(Some(output.to_vec()))
}

fn invalid_resource(message: &str) -> MetraError {
    MetraError::InvalidTag {
        context: "JPEG Photoshop resource".to_owned(),
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource(id: u16, value: &[u8]) -> Vec<u8> {
        let mut block = b"8BIM".to_vec();
        block.extend_from_slice(&id.to_be_bytes());
        block.extend_from_slice(&[0, 0]);
        block.extend_from_slice(&(value.len() as u32).to_be_bytes());
        block.extend_from_slice(value);
        if value.len() & 1 == 1 {
            block.push(0);
        }
        block
    }

    fn dataset(number: u8, value: &[u8]) -> Vec<u8> {
        let mut dataset = vec![0x1C, 2, number];
        dataset.extend_from_slice(&(value.len() as u16).to_be_bytes());
        dataset.extend_from_slice(value);
        dataset
    }

    fn app13() -> Vec<u8> {
        let mut data = PHOTOSHOP_PREFIX.to_vec();
        data.extend_from_slice(&resource(0x0400, b"preserve me"));
        data.extend_from_slice(&resource(0x0404, &dataset(25, b"before")));
        data
    }

    #[test]
    fn rewrites_iptc_dataset_and_preserves_other_resources() {
        let action = set_action("Keywords", "after", ParseLimits::default()).unwrap();
        let output = rewrite_photoshop_app13(&app13(), &action, ParseLimits::default())
            .unwrap()
            .unwrap();
        assert!(
            output
                .windows(b"preserve me".len())
                .any(|window| window == b"preserve me")
        );
        assert!(
            output
                .windows(b"after".len())
                .any(|window| window == b"after")
        );
        assert!(
            !output
                .windows(b"before".len())
                .any(|window| window == b"before")
        );
    }

    #[test]
    fn creates_a_bounded_photoshop_app13_segment() {
        let action = set_action("CaptionAbstract", "caption", ParseLimits::default()).unwrap();
        let output = new_photoshop_app13(&action, ParseLimits::default()).unwrap();
        assert_eq!(&output[..2], &[0xFF, 0xED]);
        assert!(
            output
                .windows(b"caption".len())
                .any(|window| window == b"caption")
        );
    }
}
