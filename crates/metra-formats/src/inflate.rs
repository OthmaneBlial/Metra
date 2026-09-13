use flate2::{Decompress, FlushDecompress, Status};
use metra_core::{MetraError, ParseLimits, Result};

const OUTPUT_CHUNK_BYTES: usize = 32 * 1024;

/// Decompress one zlib stream without allowing its output to exceed the
/// caller's value budget.
pub(crate) fn decompress_zlib(
    input: &[u8],
    limits: ParseLimits,
    resource: &str,
) -> Result<Vec<u8>> {
    if input.len() > limits.max_metadata_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: resource.to_owned(),
            limit: limits.max_metadata_bytes,
        });
    }

    let mut decompressor = Decompress::new(true);
    let mut output = Vec::new();
    let mut input_offset = 0_usize;
    loop {
        let input_before = decompressor.total_in();
        let output_before = decompressor.total_out();
        let mut buffer = [0_u8; OUTPUT_CHUNK_BYTES];
        let status = decompressor
            .decompress(
                input.get(input_offset..).unwrap_or_default(),
                &mut buffer,
                FlushDecompress::Finish,
            )
            .map_err(|error| MetraError::InvalidTag {
                context: resource.to_owned(),
                message: format!("zlib decompression failed: {error}"),
            })?;
        let consumed = usize::try_from(decompressor.total_in().saturating_sub(input_before))
            .map_err(|_| MetraError::InvalidOffset {
                context: format!("{resource} compressed input"),
                offset: decompressor.total_in(),
            })?;
        let produced = usize::try_from(decompressor.total_out().saturating_sub(output_before))
            .map_err(|_| MetraError::InvalidOffset {
                context: format!("{resource} decompressed output"),
                offset: decompressor.total_out(),
            })?;
        input_offset = input_offset
            .checked_add(consumed)
            .ok_or(MetraError::InvalidOffset {
                context: format!("{resource} compressed input"),
                offset: input_offset as u64,
            })?;
        let output_end =
            output
                .len()
                .checked_add(produced)
                .ok_or(MetraError::ResourceLimitExceeded {
                    resource: resource.to_owned(),
                    limit: limits.max_value_bytes,
                })?;
        if output_end > limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: resource.to_owned(),
                limit: limits.max_value_bytes,
            });
        }
        output.extend_from_slice(&buffer[..produced]);

        if status == Status::StreamEnd {
            if input_offset != input.len() {
                return Err(MetraError::InvalidTag {
                    context: resource.to_owned(),
                    message: "zlib stream has trailing bytes".to_owned(),
                });
            }
            return Ok(output);
        }
        if consumed == 0 && produced == 0 {
            return Err(MetraError::InvalidTag {
                context: resource.to_owned(),
                message: "zlib stream ended before StreamEnd".to_owned(),
            });
        }
    }
}
