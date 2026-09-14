use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

struct TemporaryDirectory {
    path: PathBuf,
}

static NEXT_DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

impl TemporaryDirectory {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after the Unix epoch")
            .as_nanos();
        let counter = NEXT_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "metra-cli-test-{}-{unique}-{counter}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary test directory should be creatable");
        Self { path }
    }

    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.path.join(name);
        fs::write(&path, bytes).expect("fixture should be writable");
        path
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn minimal_exif_jpeg() -> Vec<u8> {
    let mut tiff = vec![
        b'I', b'I', 42, 0, 8, 0, 0, 0, // little-endian TIFF header
        1, 0, // one IFD0 entry
        0x0F, 0x01, 2, 0, 5, 0, 0, 0, 26, 0, 0, 0, // Make -> offset 26
        0, 0, 0, 0, // no next IFD
    ];
    assert_eq!(tiff.len(), 26);
    tiff.extend_from_slice(b"Sony\0");

    let mut exif = b"Exif\0\0".to_vec();
    exif.extend_from_slice(&tiff);
    let segment_length = u16::try_from(exif.len() + 2).expect("fixture fits in a JPEG segment");
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
    jpeg.extend_from_slice(&segment_length.to_be_bytes());
    jpeg.extend_from_slice(&exif);
    jpeg.extend_from_slice(&[0xFF, 0xD9]);
    jpeg
}

fn minimal_exif_jpeg_with_make(make: &str) -> Vec<u8> {
    let mut tiff = vec![
        b'I', b'I', 42, 0, 8, 0, 0, 0, // little-endian TIFF header
        1, 0, // one IFD0 entry
        0x0F, 0x01, 2, 0,
    ];
    let count = u32::try_from(make.len() + 1).expect("fixture value should fit");
    tiff.extend_from_slice(&count.to_le_bytes());
    tiff.extend_from_slice(&26_u32.to_le_bytes());
    tiff.extend_from_slice(&[0, 0, 0, 0]);
    assert_eq!(tiff.len(), 26);
    tiff.extend_from_slice(make.as_bytes());
    tiff.push(0);

    let mut exif = b"Exif\0\0".to_vec();
    exif.extend_from_slice(&tiff);
    let segment_length = u16::try_from(exif.len() + 2).expect("fixture fits in a JPEG segment");
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
    jpeg.extend_from_slice(&segment_length.to_be_bytes());
    jpeg.extend_from_slice(&exif);
    jpeg.extend_from_slice(&[0xFF, 0xD9]);
    jpeg
}

fn jpeg_iptc_dataset(dataset: u8, value: &str) -> Vec<u8> {
    let value = value.as_bytes();
    let mut bytes = vec![0x1C, 2, dataset];
    bytes.extend_from_slice(&(value.len() as u16).to_be_bytes());
    bytes.extend_from_slice(value);
    bytes
}

fn jpeg_iptc_resource(datasets: &[Vec<u8>]) -> Vec<u8> {
    let mut iptc = Vec::new();
    for dataset in datasets {
        iptc.extend_from_slice(dataset);
    }

    let mut resource = b"Photoshop 3.0\0".to_vec();
    resource.extend_from_slice(b"8BIM");
    resource.extend_from_slice(&0x0404_u16.to_be_bytes());
    resource.extend_from_slice(&[0, 0]);
    resource.extend_from_slice(&(iptc.len() as u32).to_be_bytes());
    resource.extend_from_slice(&iptc);
    if iptc.len() & 1 == 1 {
        resource.push(0);
    }
    resource
}

fn minimal_iptc_jpeg(keywords: &[&str], caption: &str) -> Vec<u8> {
    let mut datasets = keywords
        .iter()
        .map(|value| jpeg_iptc_dataset(25, value))
        .collect::<Vec<_>>();
    datasets.push(jpeg_iptc_dataset(120, caption));
    let app13 = jpeg_iptc_resource(&datasets);
    let segment_length = u16::try_from(app13.len() + 2).expect("fixture fits in a JPEG segment");
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xED];
    jpeg.extend_from_slice(&segment_length.to_be_bytes());
    jpeg.extend_from_slice(&app13);
    jpeg.extend_from_slice(&[0xFF, 0xD9]);
    jpeg
}

fn minimal_jpeg_xmp(format: &str) -> Vec<u8> {
    let packet = format!(
        "<x:xmpmeta><rdf:RDF><rdf:Description xmlns:dc=\"urn:dc\" dc:format=\"{format}\"/></rdf:RDF></x:xmpmeta>"
    );
    let mut app1 = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
    app1.extend_from_slice(packet.as_bytes());
    let segment_length = u16::try_from(app1.len() + 2).expect("fixture fits in a JPEG segment");
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
    jpeg.extend_from_slice(&segment_length.to_be_bytes());
    jpeg.extend_from_slice(&app1);
    jpeg.extend_from_slice(&[0xFF, 0xD9]);
    jpeg
}

fn minimal_svg() -> Vec<u8> {
    br#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" width="32" height="24"><title>CLI vector</title></svg>"#
        .to_vec()
}

fn minimal_pdf(title: &str) -> Vec<u8> {
    format!(
        "%PDF-1.7\n5 0 obj\n<< /Title ({title}) /Author <FEFF004F0074> >>\nendobj\ntrailer\n<< /Info 5 0 R >>\nstartxref\n9\n%%EOF\n"
    )
    .into_bytes()
}

fn minimal_psd_with_xmp(format: &str) -> Vec<u8> {
    let xmp = xmp_packet(format).into_bytes();
    let mut resource = b"8BIM".to_vec();
    resource.extend_from_slice(&0x0424_u16.to_be_bytes());
    resource.extend_from_slice(&[0, 0]);
    resource.extend_from_slice(&(xmp.len() as u32).to_be_bytes());
    resource.extend_from_slice(&xmp);
    if xmp.len() % 2 == 1 {
        resource.push(0);
    }
    let mut bytes = vec![0_u8; 26];
    bytes[..4].copy_from_slice(b"8BPS");
    bytes[4..6].copy_from_slice(&1_u16.to_be_bytes());
    bytes[12..14].copy_from_slice(&3_u16.to_be_bytes());
    bytes[14..18].copy_from_slice(&100_u32.to_be_bytes());
    bytes[18..22].copy_from_slice(&200_u32.to_be_bytes());
    bytes[22..24].copy_from_slice(&8_u16.to_be_bytes());
    bytes[24..26].copy_from_slice(&3_u16.to_be_bytes());
    bytes.extend_from_slice(&0_u32.to_be_bytes());
    bytes.extend_from_slice(&(resource.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&resource);
    bytes.extend_from_slice(&0_u32.to_be_bytes());
    bytes.extend_from_slice(&0_u16.to_be_bytes());
    bytes
}

fn xmp_packet(format: &str) -> String {
    format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF><rdf:Description xmlns:dc=\"urn:dc\" dc:format=\"{format}\"/></rdf:RDF></x:xmpmeta>"
    )
}

fn minimal_avi_with_title(title: &str) -> Vec<u8> {
    let mut info_chunk = b"INAM".to_vec();
    info_chunk.extend_from_slice(&(title.len() as u32).to_le_bytes());
    info_chunk.extend_from_slice(title.as_bytes());
    if title.len() % 2 == 1 {
        info_chunk.push(0);
    }
    let mut info_payload = b"INFO".to_vec();
    info_payload.extend_from_slice(&info_chunk);
    let mut info_list = b"LIST".to_vec();
    info_list.extend_from_slice(&(info_payload.len() as u32).to_le_bytes());
    info_list.extend_from_slice(&info_payload);
    if info_payload.len() % 2 == 1 {
        info_list.push(0);
    }
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&((4 + info_list.len()) as u32).to_le_bytes());
    bytes.extend_from_slice(b"AVI ");
    bytes.extend_from_slice(&info_list);
    bytes
}

fn minimal_webm_with_title(title: &str) -> Vec<u8> {
    fn element(id: &[u8], data: &[u8]) -> Vec<u8> {
        assert!(data.len() < 127);
        let mut output = id.to_vec();
        output.push(0x80 | data.len() as u8);
        output.extend_from_slice(data);
        output
    }

    let ebml_header = element(&[0x1A, 0x45, 0xDF, 0xA3], &element(&[0x42, 0x82], b"webm"));
    let simple_tag = [
        element(&[0x45, 0xA3], b"TITLE"),
        element(&[0x44, 0x87], title.as_bytes()),
    ]
    .concat();
    let tags = element(
        &[0x12, 0x54, 0xC3, 0x67],
        &element(&[0x73, 0x73], &element(&[0x67, 0xC8], &simple_tag)),
    );
    [ebml_header, tags].concat()
}

fn minimal_webm_with_info_title(title: &str) -> Vec<u8> {
    fn element(id: &[u8], data: &[u8]) -> Vec<u8> {
        assert!(data.len() < 127);
        let mut output = id.to_vec();
        output.push(0x80 | data.len() as u8);
        output.extend_from_slice(data);
        output
    }

    let ebml_header = element(&[0x1A, 0x45, 0xDF, 0xA3], &element(&[0x42, 0x82], b"webm"));
    let info = element(
        &[0x15, 0x49, 0xA9, 0x66],
        &element(&[0x7B, 0xA9], title.as_bytes()),
    );
    [ebml_header, info].concat()
}

fn minimal_dng_with_make(make: &str) -> Vec<u8> {
    let make_length = make.len() as u16;
    let mut bytes = vec![
        b'I',
        b'I',
        42,
        0,
        8,
        0,
        0,
        0,
        1,
        0, // one IFD0 entry
        0x0F,
        0x01,
        2,
        0,
        (make_length & 0xFF) as u8,
        (make_length >> 8) as u8,
        0,
        0,
        26,
        0,
        0,
        0,
        0,
        0,
        0,
        0, // no next IFD
    ];
    assert_eq!(bytes.len(), 26);
    bytes.extend_from_slice(make.as_bytes());
    bytes
}

fn minimal_svg_document(title: &str, description: &str, comment: &str) -> Vec<u8> {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\"><!-- {comment} --><title>{title}</title><desc>{description}</desc><rect width=\"2\" height=\"2\"/></svg>"
    )
    .into_bytes()
}

fn minimal_gif(comment: &str) -> Vec<u8> {
    let mut bytes = b"GIF89a".to_vec();
    bytes.extend_from_slice(&[2, 0, 2, 0, 0, 0, 0]);
    bytes.extend_from_slice(&[0x21, 0xFE, comment.len() as u8]);
    bytes.extend_from_slice(comment.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&[0x2C, 0, 0, 0, 0, 2, 0, 2, 0, 0]);
    bytes.extend_from_slice(&[2, 1, 0x44, 0, 0x3B]);
    bytes
}

fn png_crc(kind: &[u8; 4], data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for byte in kind.iter().chain(data.iter()) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut chunk = Vec::new();
    chunk.extend_from_slice(&(data.len() as u32).to_be_bytes());
    chunk.extend_from_slice(kind);
    chunk.extend_from_slice(data);
    chunk.extend_from_slice(&png_crc(kind, data).to_be_bytes());
    chunk
}

fn minimal_png(comment: &str) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1A\n".to_vec();
    bytes.extend_from_slice(&png_chunk(
        b"IHDR",
        &[0, 0, 2, 0, 0, 0, 2, 0, 8, 2, 0, 0, 0],
    ));
    bytes.extend_from_slice(&png_chunk(
        b"tEXt",
        format!("Comment\0{comment}").as_bytes(),
    ));
    bytes.extend_from_slice(&png_chunk(b"IEND", &[]));
    bytes
}

fn minimal_png_xmp(format: &str) -> Vec<u8> {
    let xmp = format!(
        "<x:xmpmeta><rdf:RDF><rdf:Description xmlns:dc=\"urn:dc\" dc:format=\"{format}\"/></rdf:RDF></x:xmpmeta>"
    );
    let mut itxt = b"XML:com.adobe.xmp\0\0\0\0\0".to_vec();
    itxt.extend_from_slice(xmp.as_bytes());
    let mut bytes = b"\x89PNG\r\n\x1A\n".to_vec();
    bytes.extend_from_slice(&png_chunk(
        b"IHDR",
        &[0, 0, 2, 0, 0, 0, 2, 0, 8, 2, 0, 0, 0],
    ));
    bytes.extend_from_slice(&png_chunk(b"iTXt", &itxt));
    bytes.extend_from_slice(&png_chunk(b"IDAT", &[1, 2, 3, 4]));
    bytes.extend_from_slice(&png_chunk(b"IEND", &[]));
    bytes
}

fn wav_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut chunk = kind.to_vec();
    chunk.extend_from_slice(&(data.len() as u32).to_le_bytes());
    chunk.extend_from_slice(data);
    if !data.len().is_multiple_of(2) {
        chunk.push(0);
    }
    chunk
}

fn minimal_wav(title: &str) -> Vec<u8> {
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&1_u16.to_le_bytes());
    fmt.extend_from_slice(&1_u16.to_le_bytes());
    fmt.extend_from_slice(&8_000_u32.to_le_bytes());
    fmt.extend_from_slice(&8_000_u32.to_le_bytes());
    fmt.extend_from_slice(&1_u16.to_le_bytes());
    fmt.extend_from_slice(&8_u16.to_le_bytes());
    let mut list = b"INFO".to_vec();
    list.extend_from_slice(&wav_chunk(b"INAM", format!("{title}\0").as_bytes()));
    let mut body = wav_chunk(b"fmt ", &fmt);
    body.extend_from_slice(&wav_chunk(b"LIST", &list));
    body.extend_from_slice(&wav_chunk(b"data", &[9, 8, 7, 6]));
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&((4 + body.len()) as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVE");
    bytes.extend_from_slice(&body);
    bytes
}

fn flac_block(last: bool, kind: u8, data: &[u8]) -> Vec<u8> {
    let length = u32::try_from(data.len()).expect("fixture block should fit");
    let mut block = vec![if last { 0x80 | kind } else { kind }];
    block.extend_from_slice(&length.to_be_bytes()[1..]);
    block.extend_from_slice(data);
    block
}

fn minimal_flac(title: &str) -> Vec<u8> {
    let mut streaminfo = vec![0_u8; 34];
    streaminfo[0..2].copy_from_slice(&4096_u16.to_be_bytes());
    streaminfo[2..4].copy_from_slice(&4096_u16.to_be_bytes());
    let packed = (44_100_u64 << 44) | (1_u64 << 41) | (15_u64 << 36) | 88_200;
    streaminfo[10..18].copy_from_slice(&packed.to_be_bytes());

    let vendor = b"Metra CLI";
    let comment = format!("TITLE={title}");
    let mut vorbis = (vendor.len() as u32).to_le_bytes().to_vec();
    vorbis.extend_from_slice(vendor);
    vorbis.extend_from_slice(&1_u32.to_le_bytes());
    vorbis.extend_from_slice(&(comment.len() as u32).to_le_bytes());
    vorbis.extend_from_slice(comment.as_bytes());

    let mut bytes = b"fLaC".to_vec();
    bytes.extend_from_slice(&flac_block(false, 0, &streaminfo));
    bytes.extend_from_slice(&flac_block(true, 4, &vorbis));
    bytes.extend_from_slice(&[1, 2, 3, 4]);
    bytes
}

fn ogg_page(serial: u32, sequence: u32, header_type: u8, packet: &[u8]) -> Vec<u8> {
    assert!(packet.len() < 255);
    let mut page = b"OggS".to_vec();
    page.extend_from_slice(&[0, header_type]);
    page.extend_from_slice(&0_u64.to_le_bytes());
    page.extend_from_slice(&serial.to_le_bytes());
    page.extend_from_slice(&sequence.to_le_bytes());
    page.extend_from_slice(&[0; 4]);
    page.push(1);
    page.push(packet.len() as u8);
    page.extend_from_slice(packet);
    page
}

fn minimal_ogg(title: &str) -> Vec<u8> {
    let mut identification = vec![1];
    identification.extend_from_slice(b"vorbis");
    identification.extend_from_slice(&0_u32.to_le_bytes());
    identification.push(2);
    identification.extend_from_slice(&44_100_u32.to_le_bytes());
    identification.extend_from_slice(&[0; 12]);
    identification.extend_from_slice(&[0x98, 0x88, 1, 1]);

    let vendor = b"Metra CLI";
    let comment = format!("TITLE={title}");
    let mut comments = vec![3];
    comments.extend_from_slice(b"vorbis");
    comments.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    comments.extend_from_slice(vendor);
    comments.extend_from_slice(&1_u32.to_le_bytes());
    comments.extend_from_slice(&(comment.len() as u32).to_le_bytes());
    comments.extend_from_slice(comment.as_bytes());

    let mut bytes = ogg_page(15, 0, 0x02, &identification);
    bytes.extend_from_slice(&ogg_page(15, 1, 0, &comments));
    bytes
}

fn id3_synchsafe(value: usize) -> [u8; 4] {
    [
        ((value >> 21) & 0x7F) as u8,
        ((value >> 14) & 0x7F) as u8,
        ((value >> 7) & 0x7F) as u8,
        (value & 0x7F) as u8,
    ]
}

fn id3_frame(id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut frame = id.to_vec();
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(&[0, 0]);
    frame.extend_from_slice(payload);
    frame
}

fn minimal_mp3(title: &str) -> Vec<u8> {
    let mut title_payload = vec![3];
    title_payload.extend_from_slice(title.as_bytes());
    let mut frames = id3_frame(b"TIT2", &title_payload);
    let mut comment = vec![3, b'e', b'n', b'g', 0];
    comment.extend_from_slice(b"source comment");
    frames.extend_from_slice(&id3_frame(b"COMM", &comment));
    frames.extend_from_slice(&[0; 8]);

    let mut bytes = b"ID3".to_vec();
    bytes.extend_from_slice(&[4, 0, 0]);
    bytes.extend_from_slice(&id3_synchsafe(frames.len()));
    bytes.extend_from_slice(&frames);
    bytes.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x64, 1, 2, 3, 4]);
    bytes
}

fn webp_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut chunk = kind.to_vec();
    chunk.extend_from_slice(&(data.len() as u32).to_le_bytes());
    chunk.extend_from_slice(data);
    if data.len() & 1 == 1 {
        chunk.push(0);
    }
    chunk
}

fn minimal_webp(format: &str) -> Vec<u8> {
    let xmp = format!(
        "<x:xmpmeta><rdf:RDF><rdf:Description xmlns:dc=\"urn:dc\" dc:format=\"{format}\"/></rdf:RDF></x:xmpmeta>"
    );
    let mut body = webp_chunk(b"VP8X", &[0, 0, 0, 0, 1, 0, 0, 1, 0, 0]);
    body.extend_from_slice(&webp_chunk(b"VP8 ", &[1, 2, 3]));
    body.extend_from_slice(&webp_chunk(b"XMP ", xmp.as_bytes()));
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&((4 + body.len()) as u32).to_le_bytes());
    bytes.extend_from_slice(b"WEBP");
    bytes.extend_from_slice(&body);
    bytes
}

fn ebml_element(id: &[u8], data: &[u8]) -> Vec<u8> {
    assert!(
        data.len() < 127,
        "test EBML element should fit its short size"
    );
    let mut output = id.to_vec();
    output.push(0x80 | data.len() as u8);
    output.extend_from_slice(data);
    output
}

fn minimal_webm() -> Vec<u8> {
    let ebml = ebml_element(&[0x42, 0x82], b"webm");
    let ebml_header = ebml_element(b"\x1A\x45\xDF\xA3", &ebml);

    let mut info = ebml_element(&[0x2A, 0xD7, 0xB1], &[0x0F, 0x42, 0x40]);
    info.extend_from_slice(&ebml_element(&[0x44, 0x89], &10.0_f64.to_be_bytes()));
    info.extend_from_slice(&ebml_element(&[0x7B, 0xA9], b"Metra CLI\0"));

    let mut track = ebml_element(&[0xD7], &[1]);
    track.extend_from_slice(&ebml_element(&[0x83], &[1]));
    track.extend_from_slice(&ebml_element(&[0x86], b"V_VP9"));
    let tracks = ebml_element(&[0x16, 0x54, 0xAE, 0x6B], &ebml_element(&[0xAE], &track));

    let mut simple_tag = ebml_element(&[0x45, 0xA3], b"TITLE");
    simple_tag.extend_from_slice(&ebml_element(&[0x44, 0x87], b"Sample"));
    let tags = ebml_element(
        &[0x12, 0x54, 0xC3, 0x67],
        &ebml_element(&[0x73, 0x73], &ebml_element(&[0x67, 0xC8], &simple_tag)),
    );

    let mut segment_data = ebml_element(&[0x15, 0x49, 0xA9, 0x66], &info);
    segment_data.extend_from_slice(&tracks);
    segment_data.extend_from_slice(&tags);
    [
        ebml_header,
        ebml_element(&[0x18, 0x53, 0x80, 0x67], &segment_data),
    ]
    .concat()
}

fn isobmff_box(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let size = u32::try_from(data.len() + 8).expect("test box fits");
    let mut bytes = size.to_be_bytes().to_vec();
    bytes.extend_from_slice(kind);
    bytes.extend_from_slice(data);
    bytes
}

fn minimal_isobmff(title: &str) -> Vec<u8> {
    let ftyp = isobmff_box(b"ftyp", b"isom\0\0\0\0mp42");
    let mut data = vec![0, 0, 0, 1, 0, 0, 0, 0];
    data.extend_from_slice(title.as_bytes());
    let title_kind = [0xA9, b'n', b'a', b'm'];
    let title = isobmff_box(&title_kind, &isobmff_box(b"data", &data));
    let ilst = isobmff_box(b"ilst", &title);
    let udta = isobmff_box(b"udta", &ilst);
    let moov = isobmff_box(b"moov", &udta);
    [ftyp, moov].concat()
}

fn minimal_cr3(title: &str) -> Vec<u8> {
    let ftyp = isobmff_box(b"ftyp", b"crx \0\0\0\0crx ");
    let mut data = vec![0, 0, 0, 1, 0, 0, 0, 0];
    data.extend_from_slice(title.as_bytes());
    let title_kind = [0xA9, b'n', b'a', b'm'];
    let title = isobmff_box(&title_kind, &isobmff_box(b"data", &data));
    let ilst = isobmff_box(b"ilst", &title);
    let udta = isobmff_box(b"udta", &ilst);
    let moov = isobmff_box(b"moov", &udta);
    [ftyp, moov].concat()
}

fn minimal_raw_tiff() -> Vec<u8> {
    let mut tiff = vec![
        b'I', b'I', 42, 0, 8, 0, 0, 0, // little-endian TIFF header
        1, 0, // one IFD0 entry
        0x0F, 0x01, 2, 0, 6, 0, 0, 0, 26, 0, 0, 0, // Make -> offset 26
        0, 0, 0, 0, // no next IFD
    ];
    assert_eq!(tiff.len(), 26);
    tiff.extend_from_slice(b"Canon\0");
    tiff
}

fn run(args: &[&Path]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_metra"));
    for path in args {
        command.arg(path);
    }
    command.output().expect("Metra CLI should start")
}

#[test]
fn json_output_exposes_typed_exif_tags() {
    let directory = TemporaryDirectory::new();
    let path = directory.file("camera.jpg", &minimal_exif_jpeg());
    let output = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args(["--json", path.to_str().expect("UTF-8 test path")])
        .output()
        .expect("Metra CLI should start");

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let document: Value = serde_json::from_slice(&output.stdout).expect("JSON output should parse");
    assert_eq!(document["schema_version"], 1);
    assert_eq!(document["file_info"]["format"], "JPEG");
    let make = document["tags"]
        .as_array()
        .expect("tags should be an array")
        .iter()
        .find(|tag| tag["name"] == "Make")
        .expect("EXIF Make should be present");
    assert_eq!(make["namespace"], "EXIF");
    assert_eq!(make["value"]["string"], "Sony");
}

#[test]
fn legacy_dash_tag_alias_selects_a_canonical_tag() {
    let directory = TemporaryDirectory::new();
    let path = directory.file("camera.jpg", &minimal_exif_jpeg());
    let output = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args(["-Make", path.to_str().expect("UTF-8 test path")])
        .output()
        .expect("Metra CLI should start");

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("EXIF:Make"));
    assert!(stdout.contains("Sony"));
}

#[test]
fn legacy_json_alias_keeps_the_metra_schema() {
    let directory = TemporaryDirectory::new();
    let path = directory.file("camera.jpg", &minimal_exif_jpeg());
    let output = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args(["-json", path.to_str().expect("UTF-8 test path")])
        .output()
        .expect("Metra CLI should start");

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let document: Value = serde_json::from_slice(&output.stdout).expect("JSON output should parse");
    assert_eq!(document["schema_version"], 1);
    assert_eq!(document["file_info"]["format"], "JPEG");
}

#[test]
fn jsonl_and_human_modes_are_available() {
    let directory = TemporaryDirectory::new();
    let path = directory.file("camera.jpg", &minimal_exif_jpeg());

    let jsonl = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args(["--jsonl", path.to_str().expect("UTF-8 test path")])
        .output()
        .expect("Metra CLI should start");
    assert!(jsonl.status.success());
    assert_eq!(
        jsonl
            .stdout
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .count(),
        1
    );

    let human = run(&[path.as_path()]);
    assert!(human.status.success());
    let stdout = String::from_utf8(human.stdout).expect("human output should be UTF-8");
    assert!(stdout.contains("Format: JPEG"));
    assert!(stdout.contains("EXIF:Make"));
}

#[test]
fn validate_returns_failure_for_recoverable_warnings() {
    let directory = TemporaryDirectory::new();
    let mut bytes = minimal_png("warning");
    let text_start = 8 + 25;
    let text_length = u32::from_be_bytes(
        bytes[text_start..text_start + 4]
            .try_into()
            .expect("PNG text length"),
    ) as usize;
    let crc_start = text_start + 8 + text_length;
    bytes[crc_start] ^= 0xFF;
    let path = directory.file("warning.png", &bytes);

    let regular = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([path.to_str().expect("UTF-8 test path")])
        .output()
        .expect("Metra CLI should start");
    assert!(regular.status.success(), "stderr: {:?}", regular.stderr);

    let strict = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--validate",
            "--json",
            path.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(!strict.status.success());
    let document: Value = serde_json::from_slice(&strict.stdout).expect("JSON output should parse");
    assert_eq!(document["warnings"][0]["code"], "png-crc");
}

#[test]
fn cli_limits_are_applied_to_metadata_reads() {
    let directory = TemporaryDirectory::new();
    let path = directory.file("bounded.jpg", &minimal_exif_jpeg());
    let output = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--validate",
            "--max-value-bytes",
            "4",
            "--json",
            path.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(!output.status.success());
    let document: Value = serde_json::from_slice(&output.stdout).expect("JSON output should parse");
    assert!(
        document["warnings"]
            .as_array()
            .expect("warnings should be an array")
            .iter()
            .any(|warning| warning["code"] == "value-limit")
    );
}

#[test]
fn compare_reports_value_changes_and_success_for_equal_metadata() {
    let directory = TemporaryDirectory::new();
    let reference = directory.file("reference.png", &minimal_png("reference"));
    let changed = directory.file("changed.png", &minimal_png("changed"));

    let output = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--compare",
            reference.to_str().expect("UTF-8 test path"),
            changed.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(!output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("diff output should be UTF-8");
    assert!(stdout.contains("changed: PNG:Text:Comment"));

    let equal = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--compare",
            reference.to_str().expect("UTF-8 test path"),
            reference.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(equal.status.success(), "stderr: {:?}", equal.stderr);
    assert!(
        String::from_utf8(equal.stdout)
            .expect("diff output should be UTF-8")
            .contains("no metadata differences")
    );
}

#[test]
fn jobs_keep_batch_output_in_path_order() {
    let directory = TemporaryDirectory::new();
    let first = directory.file("camera-a.jpg", &minimal_exif_jpeg());
    let second = directory.file("camera-b.jpg", &minimal_exif_jpeg());
    let output = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--jsonl",
            "--jobs",
            "2",
            second.to_str().expect("UTF-8 test path"),
            first.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("JSONL output should be UTF-8");
    let mut lines = stdout.lines();
    assert!(
        lines
            .next()
            .is_some_and(|line| line.contains("camera-a.jpg"))
    );
    assert!(
        lines
            .next()
            .is_some_and(|line| line.contains("camera-b.jpg"))
    );
    assert!(lines.next().is_none());
}

#[test]
fn csv_output_contains_typed_tag_rows() {
    let directory = TemporaryDirectory::new();
    let path = directory.file("camera.csv.jpg", &minimal_exif_jpeg());
    let output = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args(["--csv", path.to_str().expect("UTF-8 test path")])
        .output()
        .expect("Metra CLI should start");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("CSV output should be UTF-8");
    assert!(
        stdout
            .lines()
            .next()
            .is_some_and(|line| { line == "path,format,namespace,group,id,name,value_type,value" })
    );
    assert!(stdout.contains("\"EXIF\""));
    assert!(stdout.contains("\"Make\""));
    assert!(stdout.contains("\"String\""));
}

#[test]
fn toml_and_yaml_outputs_keep_schema_version() {
    let directory = TemporaryDirectory::new();
    let path = directory.file("camera.jpg", &minimal_exif_jpeg());
    for format in ["--toml", "--yaml"] {
        let output = Command::new(env!("CARGO_BIN_EXE_metra"))
            .args([format, path.to_str().expect("UTF-8 test path")])
            .output()
            .expect("Metra CLI should start");
        assert!(output.status.success(), "stderr: {:?}", output.stderr);
        let text = String::from_utf8(output.stdout).expect("structured output should be UTF-8");
        assert!(text.contains("schema_version"));
        assert!(text.contains("Sony"));
    }
}

#[test]
fn svg_json_output_exposes_document_metadata() {
    let directory = TemporaryDirectory::new();
    let path = directory.file("drawing.svg", &minimal_svg());
    let output = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args(["--json", path.to_str().expect("UTF-8 test path")])
        .output()
        .expect("Metra CLI should start");

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let document: Value = serde_json::from_slice(&output.stdout).expect("JSON output should parse");
    assert_eq!(document["file_info"]["format"], "SVG");
    assert_eq!(document["file_info"]["mime_type"], "image/svg+xml");
    assert!(
        document["tags"]
            .as_array()
            .expect("tags should be an array")
            .iter()
            .any(|tag| tag["name"] == "Title" && tag["value"]["string"] == "CLI vector")
    );
}

#[test]
fn webm_json_output_exposes_bounded_ebml_metadata() {
    let directory = TemporaryDirectory::new();
    let path = directory.file("sample.webm", &minimal_webm());
    let output = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args(["--json", path.to_str().expect("UTF-8 test path")])
        .output()
        .expect("Metra CLI should start");

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let document: Value = serde_json::from_slice(&output.stdout).expect("JSON output should parse");
    assert_eq!(document["file_info"]["format"], "WEBM");
    assert_eq!(document["file_info"]["mime_type"], "video/webm");
    let tags = document["tags"]
        .as_array()
        .expect("tags should be an array");
    assert!(
        tags.iter()
            .any(|tag| tag["name"] == "Title" && tag["value"]["string"] == "Metra CLI")
    );
    assert!(
        tags.iter()
            .any(|tag| tag["name"] == "TrackNumber" && tag["value"]["unsigned"] == 1)
    );
    assert!(
        tags.iter()
            .any(|tag| tag["name"] == "Tag:TITLE" && tag["value"]["string"] == "Sample")
    );
}

#[test]
fn dng_path_uses_raw_identity_and_tiff_metadata() {
    let directory = TemporaryDirectory::new();
    let path = directory.file("capture.dng", &minimal_raw_tiff());
    let output = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args(["--json", path.to_str().expect("UTF-8 test path")])
        .output()
        .expect("Metra CLI should start");

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let document: Value = serde_json::from_slice(&output.stdout).expect("JSON output should parse");
    assert_eq!(document["file_info"]["format"], "RAW");
    assert_eq!(document["file_info"]["mime_type"], "image/x-raw");
    let tags = document["tags"]
        .as_array()
        .expect("tags should be an array");
    assert!(
        tags.iter()
            .any(|tag| tag["name"] == "Variant" && tag["value"]["string"] == "DNG")
    );
    assert!(
        tags.iter()
            .any(|tag| tag["name"] == "Make" && tag["namespace"] == "EXIF")
    );
}

#[test]
fn cli_can_create_minimal_tiff_seed_without_overwrite() {
    let directory = TemporaryDirectory::new();
    let path = directory.path.join("created.tif");
    let output = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--create-tiff",
            "EXIF:Make=Metra",
            "--create-tiff",
            "EXIF:Artist=Othmane",
            path.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let metadata = metra::read(&path).expect("created TIFF should remain readable");
    assert_eq!(metadata.file_info.format, metra::FileFormat::Tiff);
    assert_eq!(metadata.find("EXIF:Make").unwrap().display_value(), "Metra");
    assert_eq!(
        metadata.find("EXIF:Artist").unwrap().display_value(),
        "Othmane"
    );

    let second = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--create-tiff",
            "EXIF:Make=Other",
            path.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("refusing to overwrite"));
}

#[test]
fn cli_can_create_minimal_png_seed_without_overwrite() {
    let directory = TemporaryDirectory::new();
    let path = directory.path.join("created.png");
    let output = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--create-png",
            "Comment=Metra",
            "--create-png",
            "Author=Othmane",
            path.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let metadata = metra::read(&path).expect("created PNG should remain readable");
    assert_eq!(metadata.file_info.format, metra::FileFormat::Png);
    assert_eq!(
        metadata.find("PNG:Text:Comment").unwrap().display_value(),
        "Metra"
    );
    assert_eq!(
        metadata.find("PNG:Text:Author").unwrap().display_value(),
        "Othmane"
    );

    let second = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--create-png",
            "Comment=Other",
            path.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("refusing to overwrite"));
}

#[test]
fn cli_can_create_minimal_wav_seed_without_overwrite() {
    let directory = TemporaryDirectory::new();
    let path = directory.path.join("created.wav");
    let output = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--create-wav",
            "Title=Metra",
            "--create-wav",
            "Artist=Othmane",
            path.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let metadata = metra::read(&path).expect("created WAV should remain readable");
    assert_eq!(metadata.file_info.format, metra::FileFormat::Wav);
    assert_eq!(metadata.find("WAV:Title").unwrap().display_value(), "Metra");
    assert_eq!(
        metadata.find("WAV:Artist").unwrap().display_value(),
        "Othmane"
    );

    let second = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--create-wav",
            "Title=Other",
            path.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("refusing to overwrite"));
}

#[test]
fn cli_can_create_minimal_flac_seed_without_overwrite() {
    let directory = TemporaryDirectory::new();
    let path = directory.path.join("created.flac");
    let output = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--create-flac",
            "TITLE=Metra",
            "--create-flac",
            "ARTIST=Othmane",
            path.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let metadata = metra::read(&path).expect("created FLAC should remain readable");
    assert_eq!(metadata.file_info.format, metra::FileFormat::Flac);
    assert_eq!(
        metadata.find("FLAC:Title").unwrap().display_value(),
        "Metra"
    );
    assert_eq!(
        metadata.find("FLAC:Artist").unwrap().display_value(),
        "Othmane"
    );

    let second = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--create-flac",
            "TITLE=Other",
            path.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("refusing to overwrite"));
}

#[test]
fn cli_can_create_minimal_gif_seed_without_overwrite() {
    let directory = TemporaryDirectory::new();
    let path = directory.path.join("created.gif");
    let output = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--create-gif",
            "Metra",
            "--create-gif",
            "Othmane",
            path.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let metadata = metra::read(&path).expect("created GIF should remain readable");
    assert_eq!(metadata.file_info.format, metra::FileFormat::Gif);
    assert_eq!(metadata.find_all("GIF:Comment").len(), 2);
    assert_eq!(metadata.find_all("GIF:Comment")[0].display_value(), "Metra");

    let second = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--create-gif",
            "Other",
            path.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("refusing to overwrite"));
}

#[test]
fn cli_can_create_standalone_icc_without_overwrite() {
    let directory = TemporaryDirectory::new();
    let path = directory.path.join("created.icc");
    let output = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--create-icc",
            "Description=Metra",
            "--create-icc",
            "Copyright=Othmane",
            path.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let metadata = metra::read(&path).expect("created ICC should remain readable");
    assert_eq!(metadata.file_info.format, metra::FileFormat::Icc);
    assert_eq!(
        metadata.find("ICC:Description").unwrap().display_value(),
        "Metra"
    );
    assert_eq!(
        metadata.find("ICC:Copyright").unwrap().display_value(),
        "Othmane"
    );

    let second = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--create-icc",
            "Description=Other",
            path.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("refusing to overwrite"));
}

#[test]
fn cli_can_create_standalone_xmp_without_overwrite() {
    let directory = TemporaryDirectory::new();
    let path = directory.path.join("created.xmp");
    let packet = xmp_packet("created");
    let output = Command::new(env!("CARGO_BIN_EXE_metra"))
        .arg("--create-xmp")
        .arg(&packet)
        .arg(path.to_str().expect("UTF-8 test path"))
        .output()
        .expect("Metra CLI should start");

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let metadata = metra::read(&path).expect("created XMP should remain readable");
    assert_eq!(metadata.file_info.format, metra::FileFormat::Xmp);
    assert!(
        metadata
            .tags()
            .iter()
            .any(|tag| { tag.key() == "XMP:dc:format" && tag.display_value() == "created" })
    );

    let second = Command::new(env!("CARGO_BIN_EXE_metra"))
        .arg("--create-xmp")
        .arg(&packet)
        .arg(path.to_str().expect("UTF-8 test path"))
        .output()
        .expect("Metra CLI should start");
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("refusing to overwrite"));
}

#[test]
fn cli_can_edit_existing_tiff_ascii_in_place() {
    let directory = TemporaryDirectory::new();
    let path = directory.file("editable.tif", &minimal_raw_tiff());
    let output = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "TIFF:EXIF:Make=Sony",
            path.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let metadata = metra::read(&path).expect("edited TIFF should remain readable");
    assert_eq!(metadata.file_info.format, metra::FileFormat::Tiff);
    assert_eq!(metadata.find("EXIF:Make").unwrap().display_value(), "Sony");
}

#[test]
fn cli_can_copy_existing_tiff_ascii_from_another_tiff() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source.tif", &minimal_raw_tiff());
    let target = directory.file("target.tif", &minimal_raw_tiff());

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "TIFF:EXIF:Make=Sony",
            source.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);

    let copy_assignment = format!(
        "TIFF:EXIF:Make={}",
        source.to_str().expect("UTF-8 test path")
    );
    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            copy_assignment.as_str(),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);

    let metadata = metra::read(&target).expect("copied TIFF should remain readable");
    assert_eq!(metadata.find("EXIF:Make").unwrap().display_value(), "Sony");
}

#[test]
fn cli_can_set_and_copy_existing_pdf_info() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source.pdf", &minimal_pdf("Source"));
    let target = directory.file("target.pdf", &minimal_pdf("Target"));

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "PDF:Title=Edited",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("PDF:Title")
            .unwrap()
            .display_value(),
        "Edited"
    );

    let copy_assignment = format!("PDF:Title={}", source.to_str().expect("UTF-8 test path"));
    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            copy_assignment.as_str(),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("PDF:Title")
            .unwrap()
            .display_value(),
        "Source"
    );
}

#[test]
fn cli_can_set_and_copy_existing_psd_xmp() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source.psd", &minimal_psd_with_xmp("source"));
    let target = directory.file("target.psd", &minimal_psd_with_xmp("target"));
    let replacement = xmp_packet("edited");

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            &format!("PSD:XMP={replacement}"),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("XMP:dc:format")
            .unwrap()
            .display_value(),
        "edited"
    );

    let copy_assignment = format!("PSD:XMP={}", source.to_str().expect("UTF-8 test path"));
    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            &copy_assignment,
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("XMP:dc:format")
            .unwrap()
            .display_value(),
        "source"
    );
}

#[test]
fn cli_can_set_and_copy_existing_avi_info() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source.avi", &minimal_avi_with_title("source"));
    let target = directory.file("target.avi", &minimal_avi_with_title("target"));

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "AVI:Title=edited",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("AVI:Title")
            .unwrap()
            .display_value(),
        "edited"
    );

    let copy_assignment = format!("AVI:Title={}", source.to_str().expect("UTF-8 test path"));
    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            &copy_assignment,
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("AVI:Title")
            .unwrap()
            .display_value(),
        "source"
    );
}

#[test]
fn cli_can_delete_existing_avi_info() {
    let directory = TemporaryDirectory::new();
    let path = directory.file("editable.avi", &minimal_avi_with_title("Title"));
    let delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "AVI:Title",
            path.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(delete.status.success(), "stderr: {:?}", delete.stderr);
    assert!(metra::read(&path).unwrap().find("AVI:Title").is_none());
}

#[test]
fn cli_can_set_and_copy_existing_matroska_tag() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source.webm", &minimal_webm_with_title("source"));
    let target = directory.file("target.webm", &minimal_webm_with_title("target"));

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "Matroska:Tag:TITLE=edited",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("Matroska:Tag:TITLE")
            .unwrap()
            .display_value(),
        "edited"
    );

    let copy_assignment = format!(
        "Matroska:Tag:TITLE={}",
        source.to_str().expect("UTF-8 test path")
    );
    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            copy_assignment.as_str(),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("Matroska:Tag:TITLE")
            .unwrap()
            .display_value(),
        "source"
    );
}

#[test]
fn cli_can_set_and_copy_existing_matroska_info_title() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source-info.webm", &minimal_webm_with_info_title("source"));
    let target = directory.file("target-info.webm", &minimal_webm_with_info_title("target"));

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "Matroska:Title=edited",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("Matroska:Title")
            .unwrap()
            .display_value(),
        "edited"
    );

    let copy_assignment = format!(
        "Matroska:Title={}",
        source.to_str().expect("UTF-8 test path")
    );
    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            copy_assignment.as_str(),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("Matroska:Title")
            .unwrap()
            .display_value(),
        "source"
    );
}

#[test]
fn cli_can_delete_existing_matroska_text() {
    let directory = TemporaryDirectory::new();
    let info_path = directory.file("info.webm", &minimal_webm_with_info_title("Title"));
    let info_delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "Matroska:Title",
            info_path.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(
        info_delete.status.success(),
        "stderr: {:?}",
        info_delete.stderr
    );
    assert!(
        metra::read(&info_path)
            .unwrap()
            .find("Matroska:Title")
            .is_none()
    );

    let tag_path = directory.file("tag.webm", &minimal_webm());
    let tag_delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "Matroska:Tag:TITLE",
            tag_path.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(
        tag_delete.status.success(),
        "stderr: {:?}",
        tag_delete.stderr
    );
    assert!(
        metra::read(&tag_path)
            .unwrap()
            .find("Matroska:Tag:TITLE")
            .is_none()
    );
}

#[test]
fn cli_can_set_and_copy_existing_tiff_like_raw_ascii() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source.dng", &minimal_dng_with_make("Canon\0"));
    let target = directory.file("target.dng", &minimal_dng_with_make("Nikon\0"));

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "TIFF:EXIF:Make=Sony",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("EXIF:Make")
            .unwrap()
            .display_value(),
        "Sony"
    );

    let copy_assignment = format!(
        "TIFF:EXIF:Make={}",
        source.to_str().expect("UTF-8 test path")
    );
    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            copy_assignment.as_str(),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("EXIF:Make")
            .unwrap()
            .display_value(),
        "Canon"
    );
}

#[test]
fn cli_can_delete_existing_tiff_like_raw_ascii() {
    let directory = TemporaryDirectory::new();
    let path = directory.file("editable.dng", &minimal_dng_with_make("Canon\0"));
    let delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "TIFF:EXIF:Make",
            path.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(delete.status.success(), "stderr: {:?}", delete.stderr);
    let metadata = metra::read(&path).expect("deleted DNG should remain readable");
    assert_eq!(metadata.find("RAW:Variant").unwrap().display_value(), "DNG");
    assert!(metadata.find("EXIF:Make").is_none());
}

#[test]
fn cli_can_set_and_copy_existing_isobmff_text() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source.mp4", &minimal_isobmff("Origin"));
    let target = directory.file("target.mp4", &minimal_isobmff("Target"));

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "ISOBMFF:Title=Source",
            source.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);

    let copy_assignment = format!(
        "ISOBMFF:Title={}",
        source.to_str().expect("UTF-8 test path")
    );
    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            copy_assignment.as_str(),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);

    let metadata = metra::read(&target).expect("copied ISO-BMFF should remain readable");
    assert_eq!(
        metadata.find("ISOBMFF:Title").unwrap().display_value(),
        "Source"
    );
}

#[test]
fn cli_can_set_and_copy_existing_cr3_isobmff_text() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source.cr3", &minimal_cr3("Origin"));
    let target = directory.file("target.cr3", &minimal_cr3("Target"));

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "ISOBMFF:Title=Source",
            source.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);

    let copy_assignment = format!(
        "ISOBMFF:Title={}",
        source.to_str().expect("UTF-8 test path")
    );
    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            copy_assignment.as_str(),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);

    let metadata = metra::read(&target).expect("copied CR3 should remain readable");
    assert_eq!(metadata.find("RAW:Variant").unwrap().display_value(), "CR3");
    assert_eq!(
        metadata.find("ISOBMFF:Title").unwrap().display_value(),
        "Source"
    );
}

#[test]
fn cli_can_delete_existing_cr3_isobmff_text() {
    let directory = TemporaryDirectory::new();
    let path = directory.file("editable.cr3", &minimal_cr3("Title"));
    let delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "ISOBMFF:Title",
            path.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(delete.status.success(), "stderr: {:?}", delete.stderr);

    let metadata = metra::read(&path).expect("deleted CR3 should remain readable");
    assert_eq!(metadata.find("RAW:Variant").unwrap().display_value(), "CR3");
    assert!(metadata.find("ISOBMFF:Title").is_none());
}

#[test]
fn cli_can_set_and_delete_a_jpeg_comment_atomically() {
    let directory = TemporaryDirectory::new();
    let path = directory.file("editable.jpg", &minimal_exif_jpeg());
    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "JPEG:Comment=edited from cli",
            path.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    let after_set = metra::read(&path).expect("rewritten JPEG should remain readable");
    assert_eq!(
        after_set.find("JPEG:Comment").unwrap().display_value(),
        "edited from cli"
    );

    let delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "JPEG:Comment",
            path.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(delete.status.success(), "stderr: {:?}", delete.stderr);
    let after_delete = metra::read(&path).expect("rewritten JPEG should remain readable");
    assert!(after_delete.find("JPEG:Comment").is_none());
}

#[test]
fn cli_can_copy_a_jpeg_comment_between_files() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source.jpg", &minimal_exif_jpeg());
    let target = directory.file("target.jpg", &minimal_exif_jpeg());
    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "JPEG:Comment=copied value",
            source.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);

    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            &format!("JPEG:Comment={}", source.to_str().expect("UTF-8 test path")),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    let metadata = metra::read(&target).expect("target JPEG should remain readable");
    assert_eq!(
        metadata.find("JPEG:Comment").unwrap().display_value(),
        "copied value"
    );
}

#[test]
fn cli_can_set_and_copy_existing_jpeg_exif_ascii() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source-exif.jpg", &minimal_exif_jpeg_with_make("Canon"));
    let target = directory.file("target-exif.jpg", &minimal_exif_jpeg_with_make("Nikon"));

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "JPEG:EXIF:Make=Sony",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("EXIF:Make")
            .unwrap()
            .display_value(),
        "Sony"
    );

    let copy_assignment = format!(
        "JPEG:EXIF:Make={}",
        source.to_str().expect("UTF-8 test path")
    );
    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            copy_assignment.as_str(),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("EXIF:Make")
            .unwrap()
            .display_value(),
        "Canon"
    );
}

#[test]
fn cli_can_edit_and_copy_jpeg_iptc() {
    let directory = TemporaryDirectory::new();
    let source = directory.file(
        "source-iptc.jpg",
        &minimal_iptc_jpeg(&["source keyword"], "source caption"),
    );
    let target = directory.file(
        "target-iptc.jpg",
        &minimal_iptc_jpeg(&["target keyword"], "target caption"),
    );

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "IPTC:Keywords=edited keyword",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("IPTC:Keywords")
            .unwrap()
            .display_value(),
        "edited keyword"
    );

    let delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "IPTC:Keywords",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(delete.status.success(), "stderr: {:?}", delete.stderr);
    assert!(
        metra::read(&target)
            .unwrap()
            .find("IPTC:Keywords")
            .is_none()
    );

    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            &format!(
                "IPTC:CaptionAbstract={}",
                source.to_str().expect("UTF-8 test path")
            ),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("IPTC:CaptionAbstract")
            .unwrap()
            .display_value(),
        "source caption"
    );
}

#[test]
fn cli_can_edit_and_copy_jpeg_xmp() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source-xmp.jpg", &minimal_jpeg_xmp("source"));
    let target = directory.file("target-xmp.jpg", &minimal_jpeg_xmp("target"));
    let replacement = r#"<x:xmpmeta><rdf:RDF><rdf:Description xmlns:dc="urn:dc" dc:format="edited"/></rdf:RDF></x:xmpmeta>"#;
    let set_assignment = format!("JPEG:XMP={replacement}");

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            set_assignment.as_str(),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("XMP:dc:format")
            .unwrap()
            .display_value(),
        "edited"
    );

    let delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "JPEG:XMP",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(delete.status.success(), "stderr: {:?}", delete.stderr);
    assert!(metra::read(&target).unwrap().find("XMP:Packet").is_none());

    let copy_assignment = format!("JPEG:XMP={}", source.to_str().expect("UTF-8 test path"));
    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            copy_assignment.as_str(),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("XMP:dc:format")
            .unwrap()
            .display_value(),
        "source"
    );
}

#[test]
fn cli_can_edit_and_copy_a_png_text_chunk() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source.png", &minimal_png("source value"));
    let target = directory.file("target.png", &minimal_png("target value"));

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "PNG:Text:Comment=edited value",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("PNG:Text:Comment")
            .unwrap()
            .display_value(),
        "edited value"
    );

    let delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "PNG:Text:Comment",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(delete.status.success(), "stderr: {:?}", delete.stderr);
    assert!(
        metra::read(&target)
            .unwrap()
            .find("PNG:Text:Comment")
            .is_none()
    );

    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            &format!(
                "PNG:Text:Comment={}",
                source.to_str().expect("UTF-8 test path")
            ),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("PNG:Text:Comment")
            .unwrap()
            .display_value(),
        "source value"
    );
}

#[test]
fn cli_can_edit_and_copy_png_xmp() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source-xmp.png", &minimal_png_xmp("source"));
    let target = directory.file("target-xmp.png", &minimal_png_xmp("target"));

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "PNG:XMP=<x:xmpmeta><rdf:RDF><rdf:Description xmlns:dc=\"urn:dc\" dc:format=\"edited\"/></rdf:RDF></x:xmpmeta>",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("XMP:dc:format")
            .unwrap()
            .display_value(),
        "edited"
    );

    let delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "PNG:XMP",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(delete.status.success(), "stderr: {:?}", delete.stderr);
    assert!(metra::read(&target).unwrap().find("XMP:Packet").is_none());

    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            &format!("PNG:XMP={}", source.to_str().expect("UTF-8 test path")),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("XMP:dc:format")
            .unwrap()
            .display_value(),
        "source"
    );
}

#[test]
fn cli_can_edit_and_copy_svg_document_text() {
    let directory = TemporaryDirectory::new();
    let source = directory.file(
        "source.svg",
        &minimal_svg_document("source title", "source description", "source comment"),
    );
    let target = directory.file(
        "target.svg",
        &minimal_svg_document("target title", "target description", "target comment"),
    );

    for (key, value, expected) in [
        ("SVG:Title", "edited title", "SVG:Title"),
        ("SVG:Description", "edited description", "SVG:Description"),
        ("SVG:Comment", "edited comment", "SVG:Comment"),
    ] {
        let assignment = format!("{key}={value}");
        let set = Command::new(env!("CARGO_BIN_EXE_metra"))
            .args([
                "--set",
                &assignment,
                target.to_str().expect("UTF-8 test path"),
            ])
            .output()
            .expect("Metra CLI should start");
        assert!(set.status.success(), "stderr: {:?}", set.stderr);
        assert_eq!(
            metra::read(&target)
                .unwrap()
                .find(expected)
                .unwrap()
                .display_value(),
            value
        );
    }

    let delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "SVG:Comment",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(delete.status.success(), "stderr: {:?}", delete.stderr);
    assert!(metra::read(&target).unwrap().find("SVG:Comment").is_none());

    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            &format!("SVG:Title={}", source.to_str().expect("UTF-8 test path")),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("SVG:Title")
            .unwrap()
            .display_value(),
        "source title"
    );
}

#[test]
fn cli_can_edit_and_copy_wav_info() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source.wav", &minimal_wav("source title"));
    let target = directory.file("target.wav", &minimal_wav("target title"));

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "WAV:Title=edited title",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("WAV:Title")
            .unwrap()
            .display_value(),
        "edited title"
    );

    let delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "WAV:Title",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(delete.status.success(), "stderr: {:?}", delete.stderr);
    assert!(metra::read(&target).unwrap().find("WAV:Title").is_none());

    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            &format!("WAV:Title={}", source.to_str().expect("UTF-8 test path")),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("WAV:Title")
            .unwrap()
            .display_value(),
        "source title"
    );
}

#[test]
fn cli_can_edit_and_copy_flac_comments() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source.flac", &minimal_flac("source title"));
    let target = directory.file("target.flac", &minimal_flac("target title"));

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "FLAC:Title=edited title",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("FLAC:Title")
            .unwrap()
            .display_value(),
        "edited title"
    );

    let delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "FLAC:Title",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(delete.status.success(), "stderr: {:?}", delete.stderr);
    assert!(metra::read(&target).unwrap().find("FLAC:Title").is_none());

    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            &format!("FLAC:Title={}", source.to_str().expect("UTF-8 test path")),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("FLAC:Title")
            .unwrap()
            .display_value(),
        "source title"
    );
}

#[test]
fn cli_can_edit_and_copy_ogg_comments() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source.ogg", &minimal_ogg("source"));
    let target = directory.file("target.ogg", &minimal_ogg("target"));

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "Ogg:Title=edited",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("Ogg:Title")
            .unwrap()
            .display_value(),
        "edited"
    );

    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            &format!("Ogg:Title={}", source.to_str().expect("UTF-8 test path")),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("Ogg:Title")
            .unwrap()
            .display_value(),
        "source"
    );

    let delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "Ogg:Title",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(delete.status.success(), "stderr: {:?}", delete.stderr);
    assert!(metra::read(&target).unwrap().find("Ogg:Title").is_none());
}

#[test]
fn cli_can_edit_and_copy_id3_text() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source.mp3", &minimal_mp3("source title"));
    let target = directory.file("target.mp3", &minimal_mp3("target title"));

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "ID3:Title=edited title",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("ID3:Title")
            .unwrap()
            .display_value(),
        "edited title"
    );

    let set_comment = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "ID3:Comment=edited comment",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(
        set_comment.status.success(),
        "stderr: {:?}",
        set_comment.stderr
    );
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("ID3:Comment")
            .unwrap()
            .display_value(),
        "edited comment"
    );

    let delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "ID3:Comment",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(delete.status.success(), "stderr: {:?}", delete.stderr);
    assert!(metra::read(&target).unwrap().find("ID3:Comment").is_none());

    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            &format!("ID3:Title={}", source.to_str().expect("UTF-8 test path")),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("ID3:Title")
            .unwrap()
            .display_value(),
        "source title"
    );
}

#[test]
fn cli_can_edit_and_copy_gif_comments() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source.gif", &minimal_gif("source comment"));
    let target = directory.file("target.gif", &minimal_gif("target comment"));

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "GIF:Comment=edited comment",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("GIF:Comment")
            .unwrap()
            .display_value(),
        "edited comment"
    );

    let delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "GIF:Comment",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(delete.status.success(), "stderr: {:?}", delete.stderr);
    assert!(metra::read(&target).unwrap().find("GIF:Comment").is_none());

    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            &format!("GIF:Comment={}", source.to_str().expect("UTF-8 test path")),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("GIF:Comment")
            .unwrap()
            .display_value(),
        "source comment"
    );
}

#[test]
fn cli_can_edit_and_copy_webp_xmp() {
    let directory = TemporaryDirectory::new();
    let source = directory.file("source.webp", &minimal_webp("source"));
    let target = directory.file("target.webp", &minimal_webp("target"));

    let set = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--set",
            "WebP:XMP=<x:xmpmeta><rdf:RDF><rdf:Description xmlns:dc=\"urn:dc\" dc:format=\"edited\"/></rdf:RDF></x:xmpmeta>",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(set.status.success(), "stderr: {:?}", set.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("XMP:dc:format")
            .unwrap()
            .display_value(),
        "edited"
    );

    let delete = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--delete",
            "WebP:XMP",
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(delete.status.success(), "stderr: {:?}", delete.stderr);
    assert!(metra::read(&target).unwrap().find("XMP:Packet").is_none());

    let copy = Command::new(env!("CARGO_BIN_EXE_metra"))
        .args([
            "--copy",
            &format!("WebP:XMP={}", source.to_str().expect("UTF-8 test path")),
            target.to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Metra CLI should start");
    assert!(copy.status.success(), "stderr: {:?}", copy.stderr);
    assert_eq!(
        metra::read(&target)
            .unwrap()
            .find("XMP:dc:format")
            .unwrap()
            .display_value(),
        "source"
    );
}
