//! Injection des métadonnées GPano (XMP) dans un JPEG : segment APP1
//! inséré après SOI et l'APP0 JFIF éventuel.
//!
//! Buts :
//! - le sniff backend (`services/media_sniff.rs`) classe le fichier en
//!   `panorama` (il cherche `GPano:` + `equirectangular` dans les 128
//!   premiers Ko — notre segment est en tête) ;
//! - les visionneuses 360 externes reconnaissent la projection.
//!
//! Best-effort : toute structure JPEG inattendue → JPEG retourné tel quel.

/// Injecte les métadonnées GPano (equirectangulaire, zone couverte = image
/// entière) dans un JPEG encodé. Idempotent à l'usage (un nouvel APP1 par
/// appel — l'appelant n'injecte qu'une fois).
#[must_use]
pub fn write_gpano_xmp(jpeg: &[u8], width: u32, height: u32) -> Vec<u8> {
    // Structure minimale attendue : SOI (FFD8) [+ APP0 JFIF (FFE0)] …
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
    // APP1 : marqueur (2) + longueur (2, incluse) + header (29 avec NUL).
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

/// Packet XMP RDF/XML avec les attributs GPano (equirect, zone couverte =
/// image entière — les pôles sont pré-remplis dans les pixels).
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
