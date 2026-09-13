use metra_core::{MetraError, Result};
use quick_xml::events::BytesRef;

pub(crate) fn resolve_general_ref(reference: &BytesRef<'_>) -> Result<String> {
    let name = reference.decode().map_err(|error| MetraError::InvalidXml {
        message: error.to_string(),
    })?;
    let raw = format!("&{name};");
    let value = quick_xml::escape::unescape(&raw).map_err(|error| MetraError::InvalidXml {
        message: format!("unsupported XML entity {name}: {error}"),
    })?;
    if !value.chars().all(valid_xml_char) {
        return Err(MetraError::InvalidXml {
            message: format!("XML entity {name} resolves to a forbidden character"),
        });
    }
    Ok(value.into_owned())
}

fn valid_xml_char(character: char) -> bool {
    matches!(character, '\u{9}' | '\u{A}' | '\u{D}')
        || ('\u{20}'..='\u{D7FF}').contains(&character)
        || ('\u{E000}'..='\u{FFFD}').contains(&character)
        || ('\u{10000}'..='\u{10FFFF}').contains(&character)
}
