//! Hand-written Windows icon resource (.res), for build.rs: DIB and PNG images (with a minimal
//! deflate) and the .res file with the RT_ICONs and their RT_GROUP_ICON. No dependencies or SDK
//! tools. The app has its PNG writer for the demo's pictures, and its tests check all of it.

/// .res file with one icon (group with id 1) made of these images: (side, data from dib() or
/// png()), and the program's manifest. Starts with the empty entry that marks the format.
pub fn res(images: &[(usize, Vec<u8>)], manifest: &str) -> Vec<u8> {
    const RT_ICON: u16 = 3;
    const RT_GROUP_ICON: u16 = 14;
    const RT_MANIFEST: u16 = 24;
    let mut out = Vec::new();
    entry(&mut out, 0, 0, 0, &[]);
    for (i, (_, data)) in images.iter().enumerate() {
        entry(&mut out, RT_ICON, i as u16 + 1, 0x1010, data);
    }
    // GRPICONDIR + one GRPICONDIRENTRY per image
    let mut group = Vec::new();
    for v in [0u16, 1, images.len() as u16] {
        group.extend(v.to_le_bytes());
    }
    for (i, (n, data)) in images.iter().enumerate() {
        let side = if *n >= 256 { 0 } else { *n as u8 };
        group.extend([side, side, 0, 0]);
        group.extend(1u16.to_le_bytes()); // planes
        group.extend(32u16.to_le_bytes()); // bits per pixel
        group.extend((data.len() as u32).to_le_bytes());
        group.extend((i as u16 + 1).to_le_bytes());
    }
    entry(&mut out, RT_GROUP_ICON, 1, 0x1030, &group);
    entry(&mut out, RT_MANIFEST, 1, 0x1030, manifest.as_bytes());
    out
}

/// A .res entry: 32-byte header (numeric type and id) and the data, aligned to 4.
fn entry(out: &mut Vec<u8>, kind: u16, id: u16, flags: u16, data: &[u8]) {
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(32u32.to_le_bytes());
    out.extend(
        [0xFFFF, kind, 0xFFFF, id]
            .iter()
            .flat_map(|v: &u16| v.to_le_bytes()),
    );
    out.extend(0u32.to_le_bytes()); // DataVersion
    out.extend(flags.to_le_bytes());
    out.extend(0u16.to_le_bytes()); // neutral language
    out.extend(0u32.to_le_bytes()); // Version
    out.extend(0u32.to_le_bytes()); // Characteristics
    out.extend(data);
    out.resize(out.len().div_ceil(4) * 4, 0);
}

/// DIB icon image from top-to-bottom BGRA pixels: BITMAPINFOHEADER
/// (double height: color + mask), bottom-to-top BGRA and the 1-bit AND mask (1 =
/// transparent), also bottom to top.
pub fn dib(n: usize, px: &[u8]) -> Vec<u8> {
    let mask_row = n.div_ceil(32) * 4;
    let mut d = Vec::new();
    for v in [40u32, n as u32, 2 * n as u32] {
        d.extend(v.to_le_bytes());
    }
    d.extend(1u16.to_le_bytes());
    d.extend(32u16.to_le_bytes());
    d.extend(0u32.to_le_bytes()); // BI_RGB
    d.extend(((n * n * 4 + mask_row * n) as u32).to_le_bytes());
    d.extend([0u8; 16]);
    for y in (0..n).rev() {
        d.extend(&px[y * n * 4..(y + 1) * n * 4]);
    }
    for y in (0..n).rev() {
        let mut row = vec![0u8; mask_row];
        for x in 0..n {
            if px[(y * n + x) * 4 + 3] == 0 {
                row[x / 8] |= 0x80 >> (x % 8);
            }
        }
        d.extend(row);
    }
    d
}

/// width × height RGBA PNG from top-to-bottom BGRA pixels.
pub fn png(width: usize, height: usize, px: &[u8]) -> Vec<u8> {
    let mut ihdr = Vec::new();
    ihdr.extend((width as u32).to_be_bytes());
    ihdr.extend((height as u32).to_be_bytes());
    ihdr.extend([8, 6, 0, 0, 0]); // 8 bits, RGBA
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &zlib(&png_rows(width, height, px)));
    chunk(&mut out, b"IEND", &[]);
    out
}

/// PNG rows, each with the Sub filter: flat areas become zeros, which compress well.
fn png_rows(width: usize, height: usize, px: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(height * (width * 4 + 1));
    for y in 0..height {
        raw.push(1);
        for x in 0..width {
            let i = (y * width + x) * 4;
            let rgba = |j: usize| [px[j + 2], px[j + 1], px[j], px[j + 3]];
            let left = if x > 0 { rgba(i - 4) } else { [0; 4] };
            raw.extend(rgba(i).iter().zip(left).map(|(v, l)| v.wrapping_sub(l)));
        }
    }
    raw
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend((data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend(kind);
    out.extend(data);
    let crc = crc32(&out[start..]);
    out.extend(crc.to_be_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// zlib with a single fixed-Huffman deflate block; only looks for repeats of the previous byte
/// (distance 1), which is what the Sub filter leaves in a flat-colored icon.
fn zlib(data: &[u8]) -> Vec<u8> {
    let mut w = Bits::default();
    w.put(1, 1); // last block
    w.put(1, 2); // fixed Huffman
    let mut i = 0;
    while i < data.len() {
        w.literal(data[i] as u16);
        let mut run = 0;
        while i + 1 + run < data.len() && data[i + 1 + run] == data[i] && run < 258 {
            run += 1;
        }
        if run >= 3 {
            w.length(run);
            w.code(0, 5); // distance 1
            i += 1 + run;
        } else {
            i += 1;
        }
    }
    w.literal(256);
    let mut out = vec![0x78, 0x01];
    out.extend(w.finish());
    out.extend(adler32(data).to_be_bytes());
    out
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &v in data {
        a = (a + v as u32) % 65521;
        b = (b + a) % 65521;
    }
    b << 16 | a
}

/// Deflate repeat lengths: base and extra bits of codes 257..=285.
const LEN_BASE: [usize; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u32; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];

#[derive(Default)]
struct Bits {
    out: Vec<u8>,
    acc: u32,
    n: u32,
}

impl Bits {
    /// Loose bits, least significant first.
    fn put(&mut self, v: u32, bits: u32) {
        self.acc |= v << self.n;
        self.n += bits;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }

    /// Huffman code: most significant bit first.
    fn code(&mut self, c: u32, bits: u32) {
        for i in (0..bits).rev() {
            self.put((c >> i) & 1, 1);
        }
    }

    fn literal(&mut self, v: u16) {
        match v {
            0..=143 => self.code(0x30 + v as u32, 8),
            144..=255 => self.code(0x190 + (v as u32 - 144), 9),
            256..=279 => self.code(v as u32 - 256, 7),
            _ => self.code(0xC0 + (v as u32 - 280), 8),
        }
    }

    /// Length of a repeat (3..=258): its code and extra bits.
    fn length(&mut self, len: usize) {
        let i = LEN_BASE.iter().rposition(|&b| b <= len).unwrap();
        self.literal(257 + i as u16);
        self.put((len - LEN_BASE[i]) as u32, LEN_EXTRA[i]);
    }

    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push(self.acc as u8);
        }
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Inflates the subset that zlib() writes: one fixed-Huffman block with repeats at
    /// distance 1. Also checks the header and the adler32.
    fn inflate(z: &[u8]) -> Vec<u8> {
        assert_eq!(&z[..2], [0x78, 0x01]);
        let body = &z[2..z.len() - 4];
        let mut pos = 0usize;
        let mut bit = || {
            let b = (body[pos / 8] >> (pos % 8)) & 1;
            pos += 1;
            b as u32
        };
        assert_eq!((bit(), bit() | bit() << 1), (1, 1));
        let mut out = Vec::new();
        loop {
            // Fixed Huffman code: 7, 8 or 9 bits, most significant first
            let mut c = 0;
            for _ in 0..7 {
                c = c << 1 | bit();
            }
            let sym = if c <= 0x17 {
                256 + c
            } else {
                c = c << 1 | bit();
                match c {
                    0x30..=0xBF => c - 0x30,
                    0xC0..=0xC7 => 280 + c - 0xC0,
                    _ => 144 + ((c << 1 | bit()) - 0x190),
                }
            };
            match sym {
                0..=255 => out.push(sym as u8),
                256 => break,
                _ => {
                    let i = (sym - 257) as usize;
                    let extra = (0..LEN_EXTRA[i]).fold(0, |v, k| v | bit() << k) as usize;
                    let dist = (0..5).fold(0, |v, _| v << 1 | bit());
                    assert_eq!(dist, 0, "distance 1 only");
                    for _ in 0..LEN_BASE[i] + extra {
                        out.push(*out.last().unwrap());
                    }
                }
            }
        }
        assert_eq!(
            u32::from_be_bytes(z[z.len() - 4..].try_into().unwrap()),
            adler32(&out)
        );
        out
    }

    #[test]
    fn deflate_round_trips() {
        // Repeats of every length (also > 258) between loose bytes across the whole range
        let mut data = Vec::new();
        for len in 0..600 {
            data.push((len * 7 % 256) as u8);
            data.extend(std::iter::repeat_n((len % 251) as u8, len % 300));
        }
        assert_eq!(inflate(&zlib(&data)), data);
        assert_eq!(inflate(&zlib(&[])), Vec::<u8>::new());
    }

    #[test]
    fn png_of_the_icon() {
        let n = 64;
        let px = crate::icon::pixels(n);
        let p = png(n, n, &px);
        assert_eq!(&p[..8], b"\x89PNG\r\n\x1a\n");
        // IHDR, IDAT, IEND with their CRC (the empty IEND's is always AE 42 60 82)
        assert_eq!(
            &p[p.len() - 12..],
            [0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]
        );
        let idat_len = u32::from_be_bytes(p[33..37].try_into().unwrap()) as usize;
        assert_eq!(&p[37..41], b"IDAT");
        assert_eq!(inflate(&p[41..41 + idat_len]), png_rows(n, n, &px));
        // And it compresses: a flat icon takes much less than uncompressed
        assert!(idat_len < n * n);
    }

    #[test]
    fn a_png_can_be_wider_than_it_is_tall() {
        let px = vec![0x40; 3 * 2 * 4];
        let p = png(3, 2, &px);
        assert_eq!(u32::from_be_bytes(p[16..20].try_into().unwrap()), 3);
        assert_eq!(u32::from_be_bytes(p[20..24].try_into().unwrap()), 2);
        assert_eq!(png_rows(3, 2, &px).len(), 2 * (3 * 4 + 1));
    }

    #[test]
    fn res_layout() {
        let images = vec![
            (16, dib(16, &crate::icon::pixels(16))),
            (256, png(256, 256, &crate::icon::pixels(256))),
        ];
        let manifest = "<assembly/>";
        let r = res(&images, manifest);
        // Entries: (type, id, size), each aligned to 4
        let mut entries = Vec::new();
        let mut i = 0;
        while i < r.len() {
            let size = u32::from_le_bytes(r[i..i + 4].try_into().unwrap()) as usize;
            let word = |k: usize| u16::from_le_bytes(r[i + k..i + k + 2].try_into().unwrap());
            assert_eq!((word(8), word(12)), (0xFFFF, 0xFFFF));
            entries.push((word(10), word(14), size));
            i += (32 + size).div_ceil(4) * 4;
        }
        assert_eq!(i, r.len());
        let sizes = [images[0].1.len(), images[1].1.len()];
        assert_eq!(
            entries,
            [
                (0, 0, 0),
                (3, 1, sizes[0]),
                (3, 2, sizes[1]),
                (14, 1, 6 + 2 * 14),
                (24, 1, manifest.len())
            ]
        );
        // The DIB mask: the (transparent) corners set to 1
        let d = &images[0].1;
        assert_eq!(d.len(), 40 + 16 * 16 * 4 + 16 * 4);
        assert_eq!(d[40 + 16 * 16 * 4] & 0x80, 0x80);
    }
}
