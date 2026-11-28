//! Minimal PNG writer (uncompressed deflate), for CI smoke-test
//! screenshots. Pure Rust, no dependencies; testable anywhere.

fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    for (n, e) in t.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *e = c;
    }
    t
}

/// CRC-32 as PNG chunks use it.
pub fn crc32(parts: &[&[u8]]) -> u32 {
    let t = crc_table();
    let mut c = 0xffff_ffffu32;
    for p in parts {
        for b in *p {
            c = t[((c ^ *b as u32) & 0xff) as usize] ^ (c >> 8);
        }
    }
    c ^ 0xffff_ffff
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for x in chunk {
            a += *x as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
}

/// Encode `w`×`h` RGBA pixels (row-major, 4 bytes each) as a PNG file.
pub fn encode_rgba(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    assert_eq!(rgba.len(), (w * h * 4) as usize);
    // Raw scanlines, each prefixed with filter type 0.
    let stride = (w * 4) as usize;
    let mut raw = Vec::with_capacity((stride + 1) * h as usize);
    for row in rgba.chunks(stride) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    // zlib stream of stored (uncompressed) deflate blocks.
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    if blocks.is_empty() {
        z.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    for (i, b) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        let len = b.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(b);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA, no interlace
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_structure() {
        // Known CRC of an IEND chunk.
        assert_eq!(crc32(&[b"IEND"]), 0xae42_6082);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
        let px = [255u8, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 128];
        let png = encode_rgba(2, 2, &px);
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
    }
}
