//! Injection of GPano (XMP) metadata into a JPEG: APP1 segment
//! inserted after SOI and the optional JFIF APP0.
//!
//! Goals:
//! - the backend sniff (`services/media_sniff.rs`) classifies the file as
//!   `panorama` (it looks for `GPano:` + `equirectangular` in the first 128
//!   KB — our segment is at the head);
//! - external 360 viewers recognise the projection.
//!
//! Best-effort: any unexpected JPEG structure → JPEG returned as is.

/// Injects GPano metadata (equirectangular, covered area = whole
/// image) into an encoded JPEG. Idempotent in practice (one new APP1 per
/// call — the caller injects only once).
#[must_use]
pub fn write_gpano_xmp(jpeg: &[u8], width: u32, height: u32) -> Vec<u8> {
    // Minimal expected structure: SOI (FFD8) [+ JFIF APP0 (FFE0)] …
    if jpeg.len() < 4 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return jpeg.to_vec();
    }
    let mut insert_at = 2usize;
    if jpeg.len() >= 4 && jpeg[2] == 0xFF && jpeg[3] == 0xE0 && jpeg.len() >= 6 {
        let len = u16::from_be_bytes([jpeg[4], jpeg[5]]) as usize;
        if 2 + len > jpeg.len() - 2 {
            return jpeg.to_vec();
        }
        insert_at = 2 + 2 + len;
    }

    let payload = xmp_packet(width, height);
    // APP1: marker (2) + length (2, included) + header (29 with NUL).
    let header = b"http://ns.adobe.com/xap/1.0/\0";
    let seg_len = 2 + header.len() + payload.len();
    if seg_len > u16::MAX as usize {
        return jpeg.to_vec();
    }

    let mut out = Vec::with_capacity(jpeg.len() + seg_len + 2);
    out.extend_from_slice(&jpeg[..insert_at]);
    out.push(0xFF);
    out.push(0xE1);
    out.extend_from_slice(&(seg_len as u16).to_be_bytes());
    out.extend_from_slice(header);
    out.extend_from_slice(payload.as_bytes());
    out.extend_from_slice(&jpeg[insert_at..]);
    out
}

/// XMP RDF/XML packet with the GPano attributes (equirect, covered area =
/// whole image — the poles are pre-filled in the pixels).
fn xmp_packet(width: u32, height: u32) -> String {
    format!(
        r#"<?xpacket begin="&#xFEFF;" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:GPano="http://ns.google.com/photos/1.0/panorama/"
    GPano:ProjectionType="equirectangular"
    GPano:UsePanoramaViewer="True"
    GPano:FullPanoWidthPixels="{w}"
    GPano:FullPanoHeightPixels="{h}"
    GPano:CroppedAreaLeftPixels="0"
    GPano:CroppedAreaTopPixels="0"
    GPano:CroppedAreaImageWidthPixels="{w}"
    GPano:CroppedAreaImageHeightPixels="{h}"/>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#,
        w = width,
        h = height
    )
}
