use std::path::Path;

use metra_core::{ParseLimits, Result};

use crate::{
    AviCreateOptions, FlacCreateOptions, GifCreateOptions, IccCreateOptions, JpegCreateOptions,
    MatroskaCreateOptions, Mp3CreateOptions, OggCreateOptions, PdfCreateOptions, PngCreateOptions,
    PsdCreateOptions, SvgCreateOptions, TiffCreateOptions, WavCreateOptions, WebpCreateOptions,
    create_avi_path, create_avi_to_vec, create_flac_path, create_flac_to_vec, create_gif_path,
    create_gif_to_vec, create_icc_path, create_icc_to_vec, create_jpeg_path, create_jpeg_to_vec,
    create_matroska_path, create_matroska_to_vec, create_mp3_path, create_mp3_to_vec,
    create_ogg_path, create_ogg_to_vec, create_pdf_path, create_pdf_to_vec, create_png_path,
    create_png_to_vec, create_psd_path, create_psd_to_vec, create_svg_path, create_svg_to_vec,
    create_tiff_path, create_tiff_to_vec, create_wav_path, create_wav_to_vec, create_webp_path,
    create_webp_to_vec, create_xmp_path, create_xmp_to_vec,
};

/// Typed creation request dispatching to a format-specific validated creator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateRequest {
    Avi(AviCreateOptions),
    Matroska(MatroskaCreateOptions),
    Jpeg(JpegCreateOptions),
    Tiff(TiffCreateOptions),
    Png(PngCreateOptions),
    Webp(WebpCreateOptions),
    Gif(GifCreateOptions),
    Pdf(PdfCreateOptions),
    Psd(PsdCreateOptions),
    Mp3(Mp3CreateOptions),
    Flac(FlacCreateOptions),
    Ogg(OggCreateOptions),
    Wav(WavCreateOptions),
    Svg(SvgCreateOptions),
    Xmp(String),
    Icc(IccCreateOptions),
}

/// Create bytes through the selected format-specific validated seam.
pub fn create_to_vec(request: &CreateRequest, limits: ParseLimits) -> Result<Vec<u8>> {
    match request {
        CreateRequest::Avi(options) => create_avi_to_vec(options, limits),
        CreateRequest::Matroska(options) => create_matroska_to_vec(options, limits),
        CreateRequest::Jpeg(options) => create_jpeg_to_vec(options, limits),
        CreateRequest::Tiff(options) => create_tiff_to_vec(options, limits),
        CreateRequest::Png(options) => create_png_to_vec(options, limits),
        CreateRequest::Webp(options) => create_webp_to_vec(options, limits),
        CreateRequest::Gif(options) => create_gif_to_vec(options, limits),
        CreateRequest::Pdf(options) => create_pdf_to_vec(options, limits),
        CreateRequest::Psd(options) => create_psd_to_vec(options, limits),
        CreateRequest::Mp3(options) => create_mp3_to_vec(options, limits),
        CreateRequest::Flac(options) => create_flac_to_vec(options, limits),
        CreateRequest::Ogg(options) => create_ogg_to_vec(options, limits),
        CreateRequest::Wav(options) => create_wav_to_vec(options, limits),
        CreateRequest::Svg(options) => create_svg_to_vec(options, limits),
        CreateRequest::Xmp(packet) => create_xmp_to_vec(packet, limits),
        CreateRequest::Icc(options) => create_icc_to_vec(options, limits),
    }
}

/// Create a new path through the selected format-specific atomic seam.
pub fn create_path(
    path: impl AsRef<Path>,
    request: &CreateRequest,
    limits: ParseLimits,
) -> Result<()> {
    match request {
        CreateRequest::Avi(options) => create_avi_path(path, options, limits),
        CreateRequest::Matroska(options) => create_matroska_path(path, options, limits),
        CreateRequest::Jpeg(options) => create_jpeg_path(path, options, limits),
        CreateRequest::Tiff(options) => create_tiff_path(path, options, limits),
        CreateRequest::Png(options) => create_png_path(path, options, limits),
        CreateRequest::Webp(options) => create_webp_path(path, options, limits),
        CreateRequest::Gif(options) => create_gif_path(path, options, limits),
        CreateRequest::Pdf(options) => create_pdf_path(path, options, limits),
        CreateRequest::Psd(options) => create_psd_path(path, options, limits),
        CreateRequest::Mp3(options) => create_mp3_path(path, options, limits),
        CreateRequest::Flac(options) => create_flac_path(path, options, limits),
        CreateRequest::Ogg(options) => create_ogg_path(path, options, limits),
        CreateRequest::Wav(options) => create_wav_path(path, options, limits),
        CreateRequest::Svg(options) => create_svg_path(path, options, limits),
        CreateRequest::Xmp(packet) => create_xmp_path(path, packet, limits),
        CreateRequest::Icc(options) => create_icc_path(path, options, limits),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn generic_dispatch_creates_and_revalidates_pdf() {
        let request = CreateRequest::Pdf(PdfCreateOptions::new().with_info("Title", "Metra"));
        let bytes = create_to_vec(&request, ParseLimits::default()).unwrap();
        assert!(bytes.starts_with(b"%PDF-1.7"));
    }

    #[test]
    fn generic_dispatch_creates_standalone_xmp() {
        let request = CreateRequest::Xmp(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF/></x:xmpmeta>".to_owned(),
        );
        let bytes = create_to_vec(&request, ParseLimits::default()).unwrap();
        assert!(bytes.starts_with(b"<x:xmpmeta"));
    }

    #[test]
    fn generic_dispatch_creates_avi_seed() {
        let request = CreateRequest::Avi(AviCreateOptions::new().with_info("Title", "Metra"));
        let bytes = create_to_vec(&request, ParseLimits::default()).unwrap();
        assert!(bytes.starts_with(b"RIFF"));
    }

    #[test]
    fn generic_path_dispatch_keeps_no_overwrite_contract() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("metra-generic-create-{unique}"));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("created.pdf");
        let request = CreateRequest::Pdf(PdfCreateOptions::new());
        create_path(&path, &request, ParseLimits::default()).unwrap();
        assert!(create_path(&path, &request, ParseLimits::default()).is_err());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn generic_avi_path_dispatch_keeps_no_overwrite_contract() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("metra-generic-avi-{unique}"));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("created.avi");
        let request = CreateRequest::Avi(AviCreateOptions::new());
        create_path(&path, &request, ParseLimits::default()).unwrap();
        assert!(create_path(&path, &request, ParseLimits::default()).is_err());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn generic_matroska_path_dispatch_keeps_no_overwrite_contract() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("metra-generic-mkv-{unique}"));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("created.mkv");
        let request =
            CreateRequest::Matroska(MatroskaCreateOptions::default().with_info("Title", "Metra"));
        create_path(&path, &request, ParseLimits::default()).unwrap();
        assert!(create_path(&path, &request, ParseLimits::default()).is_err());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn generic_dispatch_creates_matroska_seed() {
        let request =
            CreateRequest::Matroska(MatroskaCreateOptions::default().with_info("Title", "Metra"));
        let bytes = create_to_vec(&request, ParseLimits::default()).unwrap();
        assert_eq!(
            crate::detect_format(&bytes).unwrap().format,
            metra_core::FileFormat::Mkv
        );
    }

    #[test]
    fn generic_dispatch_creates_psd_seed() {
        let request = CreateRequest::Psd(PsdCreateOptions::default());
        let bytes = create_to_vec(&request, ParseLimits::default()).unwrap();
        assert_eq!(
            crate::detect_format(&bytes).unwrap().format,
            metra_core::FileFormat::Psd
        );
    }
}
