use std::io::{self, Cursor, Read, Seek, SeekFrom};

struct ShortReader {
    inner: Cursor<Vec<u8>>,
    max_read: usize,
}

impl Read for ShortReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let end = buffer.len().min(self.max_read);
        self.inner.read(&mut buffer[..end])
    }
}

impl Seek for ShortReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.inner.seek(position)
    }
}

#[test]
fn public_reader_api_detects_and_dispatches_in_memory_tiff() {
    let bytes = b"II*\0\0\0\0\0";
    let mut reader = Cursor::new(bytes.as_slice());
    let metadata = metra::read_from(
        &mut reader,
        metra::FileInfo::new(
            "memory.tif".into(),
            bytes.len() as u64,
            metra::FileFormat::Unknown,
        ),
    )
    .expect("in-memory TIFF should use the public reader dispatch");

    assert_eq!(metadata.file_info.format, metra::FileFormat::Tiff);
    assert!(
        metadata
            .warnings
            .iter()
            .any(|warning| warning.code == "missing-ifd")
    );
}

#[test]
fn public_reader_api_handles_short_signature_reads() {
    let bytes = b"II*\0\0\0\0\0".to_vec();
    let mut reader = ShortReader {
        inner: Cursor::new(bytes.clone()),
        max_read: 1,
    };
    let metadata = metra::read_from(
        &mut reader,
        metra::FileInfo::new(
            "short-reader.tif".into(),
            bytes.len() as u64,
            metra::FileFormat::Jpeg,
        ),
    )
    .expect("short reads should not prevent signature detection");

    assert_eq!(metadata.file_info.format, metra::FileFormat::Tiff);
}
