//! A minimal, dependency-free PNG encoder.
//!
//! Why hand-rolled instead of the `png`/`image` crates: this crate must build in
//! the Flux workspace with no new external dependencies (the offline registry
//! cache is authoritative here), and the only thing we need is "turn RGB bytes
//! into a file a human or an agent can actually look at". That is a few hundred
//! lines of well-specified format work, so we do it honestly rather than pulling
//! an image-processing stack in for one function.
//!
//! The DEFLATE stream uses **stored (uncompressed) blocks**, which is a fully
//! valid zlib stream — every PNG decoder accepts it. It trades file size for
//! having zero compression code to get wrong. A synthetic 640×480 test frame
//! lands around 900 KB; that is irrelevant for on-demand snapshots and it means
//! the encoder cannot corrupt data through a buggy Huffman path.

use std::sync::OnceLock;

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Largest payload a single DEFLATE stored block can carry (LEN is a u16).
const MAX_STORED_BLOCK: usize = 65_535;

fn crc_table() -> &'static [u32; 256] {
    static TABLE: OnceLock<[u32; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for (n, slot) in table.iter_mut().enumerate() {
            let mut c = n as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *slot = c;
        }
        table
    })
}

/// CRC-32 as specified by PNG (ISO 3309 / ITU-T V.42).
pub fn crc32(data: &[u8]) -> u32 {
    let table = crc_table();
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c = table[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

/// Adler-32, the zlib stream checksum.
pub fn adler32(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    // 5552 is the largest run that cannot overflow the u32 accumulators.
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += byte as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

/// Wrap `data` in a zlib stream built entirely from DEFLATE stored blocks.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    // CMF=0x78 (deflate, 32K window), FLG=0x01 chosen so (CMF<<8|FLG) % 31 == 0.
    let mut out = vec![0x78, 0x01];

    if data.is_empty() {
        // A single empty, final stored block.
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0xFF, 0xFF]);
    } else {
        let chunks: Vec<&[u8]> = data.chunks(MAX_STORED_BLOCK).collect();
        let last = chunks.len() - 1;
        for (i, chunk) in chunks.iter().enumerate() {
            // We are byte-aligned at the start of every stored block, so the
            // 3 header bits (BFINAL + BTYPE=00) occupy one byte on their own.
            out.push(if i == last { 0x01 } else { 0x00 });
            let len = chunk.len() as u16;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(chunk);
        }
    }

    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    let mut crc_input = Vec::with_capacity(4 + payload.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(payload);
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// Encode raw 8-bit RGB samples (`width * height * 3` bytes) as a PNG.
///
/// Returns `None` if `rgb` is not exactly the right length — an explicit
/// mismatch is a programming error we refuse to paper over by padding.
pub fn encode_rgb(width: u32, height: u32, rgb: &[u8]) -> Option<Vec<u8>> {
    let expected = (width as usize).checked_mul(height as usize)?.checked_mul(3)?;
    if rgb.len() != expected || width == 0 || height == 0 {
        return None;
    }

    // Each scanline is prefixed with filter byte 0 (None).
    let stride = width as usize * 3;
    let mut raw = Vec::with_capacity(height as usize * (stride + 1));
    for row in rgb.chunks(stride) {
        raw.push(0);
        raw.extend_from_slice(row);
    }

    let mut out = Vec::with_capacity(raw.len() + 128);
    out.extend_from_slice(&PNG_SIGNATURE);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(2); // colour type 2 = truecolour RGB
    ihdr.push(0); // compression method (deflate)
    ihdr.push(0); // filter method
    ihdr.push(0); // interlace: none
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    chunk(&mut out, b"IEND", &[]);

    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adler32_matches_known_vector() {
        // zlib's documented example: adler32("Wikipedia") == 0x11E60398
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn crc32_matches_known_vector() {
        // The canonical CRC-32 check value for "123456789".
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn encodes_a_structurally_valid_png() {
        let png = encode_rgb(2, 2, &[255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255])
            .expect("2x2 RGB should encode");
        assert_eq!(&png[..8], &PNG_SIGNATURE, "signature must lead the file");
        assert_eq!(&png[12..16], b"IHDR", "IHDR must be the first chunk");
        assert!(png.ends_with(&[0xAE, 0x42, 0x60, 0x82]), "IEND CRC must terminate");
        // Every chunk CRC must verify when walked independently.
        assert!(walk_and_verify(&png), "all chunk CRCs must check out");
    }

    #[test]
    fn rejects_wrong_buffer_length() {
        assert!(encode_rgb(4, 4, &[0u8; 10]).is_none());
        assert!(encode_rgb(0, 4, &[]).is_none());
    }

    #[test]
    fn survives_multi_block_payloads() {
        // 200x200 RGB => 120,600 raw bytes => spans >1 stored block (65,535 cap),
        // which is exactly where a naive single-block encoder silently truncates.
        let w = 200u32;
        let h = 200u32;
        let rgb = vec![7u8; (w * h * 3) as usize];
        let png = encode_rgb(w, h, &rgb).expect("large frame should encode");
        assert!(walk_and_verify(&png), "multi-block PNG must stay CRC-valid");
    }

    /// Walk the chunk structure and verify every CRC — a real structural check,
    /// not just "the function returned some bytes".
    fn walk_and_verify(png: &[u8]) -> bool {
        let mut i = 8;
        while i + 8 <= png.len() {
            let len = u32::from_be_bytes([png[i], png[i + 1], png[i + 2], png[i + 3]]) as usize;
            let kind_and_data = &png[i + 4..i + 8 + len];
            let stored = u32::from_be_bytes([
                png[i + 8 + len],
                png[i + 9 + len],
                png[i + 10 + len],
                png[i + 11 + len],
            ]);
            if crc32(kind_and_data) != stored {
                return false;
            }
            i += 12 + len;
        }
        i == png.len()
    }
}
