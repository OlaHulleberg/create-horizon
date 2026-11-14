//! Gzip as `NbtIo.readCompressed` and `writeCompressed` read and write it (RFC 1952 around
//! RFC 1951), for the schematic files Create keeps in `schematics/`: the mod API reads and
//! writes a file's bytes but has no compression of its own.
//!
//! `decode` inflates any stream Java's `GZIPOutputStream` (or any other gzip writer) makes:
//! stored, fixed and dynamic Huffman blocks, several members' worth of header flags, and the
//! trailer's CRC-32 and length checked. `encode` writes stored blocks, which every inflater
//! reads; the files are as large as their NBT, well within `maxTotalSchematicSize`'s bounds
//! for what Create's own export makes.

/// Why a gzip stream could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GzipError {
    /// Not a gzip stream (its magic or method), or cut short.
    Header,
    /// A malformed deflate block.
    Data,
    /// More bytes than the caller allows.
    TooLarge,
    /// The trailer's CRC-32 or length does not match what was read.
    Check,
}

/// CRC-32 (IEEE, reflected) of `bytes`, as the gzip trailer carries it.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// `bytes` as one gzip member of stored deflate blocks.
pub fn encode(bytes: &[u8]) -> Vec<u8> {
    let blocks = bytes.len() / 0xFFFF + 1;
    let mut out = Vec::with_capacity(bytes.len() + blocks * 5 + 18);
    // Magic, deflate, no flags, no time, no extra flags, an unknown system.
    out.extend_from_slice(&[0x1F, 0x8B, 8, 0, 0, 0, 0, 0, 0, 0xFF]);
    let mut chunks = bytes.chunks(0xFFFF).peekable();
    if chunks.peek().is_none() {
        out.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    while let Some(chunk) = chunks.next() {
        let last = chunks.peek().is_none();
        let len = chunk.len() as u16;
        out.push(u8::from(last));
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    out.extend_from_slice(&crc32(bytes).to_le_bytes());
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out
}

/// The bytes of a gzip stream, at most `max` of them; concatenated members are read in turn,
/// as `GZIPInputStream` does.
pub fn decode(data: &[u8], max: usize) -> Result<Vec<u8>, GzipError> {
    let mut out = Vec::new();
    let mut at = 0;
    loop {
        at = member(data, at, max, &mut out)?;
        if data.len() - at < 2 || data[at] != 0x1F || data[at + 1] != 0x8B {
            return Ok(out);
        }
    }
}

/// One member from `at`, appended to `out`: where the next begins.
fn member(data: &[u8], at: usize, max: usize, out: &mut Vec<u8>) -> Result<usize, GzipError> {
    let head = data.get(at..at + 10).ok_or(GzipError::Header)?;
    if head[0] != 0x1F || head[1] != 0x8B || head[2] != 8 {
        return Err(GzipError::Header);
    }
    let flags = head[3];
    let mut p = at + 10;
    // FEXTRA, FNAME, FCOMMENT, FHCRC.
    if flags & 4 != 0 {
        let len = data.get(p..p + 2).ok_or(GzipError::Header)?;
        p += 2 + usize::from(u16::from_le_bytes([len[0], len[1]]));
    }
    for bit in [8, 16] {
        if flags & bit != 0 {
            let end = data
                .get(p..)
                .and_then(|rest| rest.iter().position(|&b| b == 0))
                .ok_or(GzipError::Header)?;
            p += end + 1;
        }
    }
    if flags & 2 != 0 {
        p += 2;
    }
    let start = out.len();
    let mut bits = Bits {
        data,
        pos: p,
        bit: 0,
        hold: 0,
    };
    inflate(&mut bits, out, max)?;
    let p = bits.pos;
    let trailer = data.get(p..p + 8).ok_or(GzipError::Header)?;
    let crc = u32::from_le_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
    let len = u32::from_le_bytes([trailer[4], trailer[5], trailer[6], trailer[7]]);
    let read = &out[start..];
    if crc32(read) != crc || read.len() as u32 != len {
        return Err(GzipError::Check);
    }
    Ok(p + 8)
}

/// A deflate stream's bits, least significant first.
struct Bits<'a> {
    data: &'a [u8],
    /// The next byte not yet in `hold`.
    pos: usize,
    /// How many bits `hold` has.
    bit: u32,
    hold: u32,
}

impl Bits<'_> {
    fn need(&mut self, n: u32) -> Result<(), GzipError> {
        while self.bit < n {
            let b = *self.data.get(self.pos).ok_or(GzipError::Data)?;
            self.pos += 1;
            self.hold |= u32::from(b) << self.bit;
            self.bit += 8;
        }
        Ok(())
    }

    fn take(&mut self, n: u32) -> Result<u32, GzipError> {
        if n == 0 {
            return Ok(0);
        }
        self.need(n)?;
        let v = self.hold & ((1u32 << n) - 1);
        self.hold >>= n;
        self.bit -= n;
        Ok(v)
    }

    /// Drops the bits left of the current byte (a stored block starts on a byte).
    fn align(&mut self) {
        self.hold = 0;
        self.bit = 0;
    }
}

/// A canonical Huffman code: how many codes of each length, and the symbols in code order.
struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Result<Huffman, GzipError> {
        let mut counts = [0u16; 16];
        for &l in lengths {
            counts[usize::from(l)] += 1;
        }
        counts[0] = 0;
        // An over-subscribed code is malformed; an incomplete one is allowed (a single code).
        let mut left: i32 = 1;
        for &c in &counts[1..] {
            left = (left << 1) - i32::from(c);
            if left < 0 {
                return Err(GzipError::Data);
            }
        }
        let mut offsets = [0u16; 16];
        for len in 1..15 {
            offsets[len + 1] = offsets[len] + counts[len];
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (s, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[usize::from(offsets[usize::from(l)])] = s as u16;
                offsets[usize::from(l)] += 1;
            }
        }
        Ok(Huffman { counts, symbols })
    }

    fn decode(&self, bits: &mut Bits<'_>) -> Result<u16, GzipError> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= bits.take(1)? as i32;
            let count = i32::from(self.counts[len]);
            if code - count < first {
                return self
                    .symbols
                    .get((index + (code - first)) as usize)
                    .copied()
                    .ok_or(GzipError::Data);
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err(GzipError::Data)
    }
}

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

fn inflate(bits: &mut Bits<'_>, out: &mut Vec<u8>, max: usize) -> Result<(), GzipError> {
    loop {
        let last = bits.take(1)? == 1;
        match bits.take(2)? {
            0 => {
                bits.align();
                let head = bits
                    .data
                    .get(bits.pos..bits.pos + 4)
                    .ok_or(GzipError::Data)?;
                let len = u16::from_le_bytes([head[0], head[1]]);
                let nlen = u16::from_le_bytes([head[2], head[3]]);
                if len != !nlen {
                    return Err(GzipError::Data);
                }
                let from = bits.pos + 4;
                let block = bits
                    .data
                    .get(from..from + usize::from(len))
                    .ok_or(GzipError::Data)?;
                if out.len() + block.len() > max {
                    return Err(GzipError::TooLarge);
                }
                out.extend_from_slice(block);
                bits.pos = from + usize::from(len);
            }
            1 => {
                let mut lengths = [0u8; 288];
                lengths[..144].fill(8);
                lengths[144..256].fill(9);
                lengths[256..280].fill(7);
                lengths[280..].fill(8);
                let lit = Huffman::new(&lengths)?;
                let dist = Huffman::new(&[5u8; 30])?;
                codes(bits, out, &lit, &dist, max)?;
            }
            2 => {
                let (lit, dist) = dynamic(bits)?;
                codes(bits, out, &lit, &dist, max)?;
            }
            _ => return Err(GzipError::Data),
        }
        if last {
            // The trailer starts on the next byte.
            bits.align();
            return Ok(());
        }
    }
}

fn dynamic(bits: &mut Bits<'_>) -> Result<(Huffman, Huffman), GzipError> {
    const ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    let nlen = bits.take(5)? as usize + 257;
    let ndist = bits.take(5)? as usize + 1;
    let ncode = bits.take(4)? as usize + 4;
    if nlen > 286 || ndist > 30 {
        return Err(GzipError::Data);
    }
    let mut code_lengths = [0u8; 19];
    for &i in &ORDER[..ncode] {
        code_lengths[i] = bits.take(3)? as u8;
    }
    let code = Huffman::new(&code_lengths)?;
    let mut lengths = vec![0u8; nlen + ndist];
    let mut i = 0;
    while i < nlen + ndist {
        let sym = code.decode(bits)?;
        let (value, repeat) = match sym {
            0..=15 => (sym as u8, 1),
            16 => {
                let prev = *lengths.get(i.wrapping_sub(1)).ok_or(GzipError::Data)?;
                (prev, 3 + bits.take(2)? as usize)
            }
            17 => (0, 3 + bits.take(3)? as usize),
            18 => (0, 11 + bits.take(7)? as usize),
            _ => return Err(GzipError::Data),
        };
        if i + repeat > lengths.len() {
            return Err(GzipError::Data);
        }
        lengths[i..i + repeat].fill(value);
        i += repeat;
    }
    if lengths[256] == 0 {
        return Err(GzipError::Data);
    }
    Ok((
        Huffman::new(&lengths[..nlen])?,
        Huffman::new(&lengths[nlen..])?,
    ))
}

fn codes(
    bits: &mut Bits<'_>,
    out: &mut Vec<u8>,
    lit: &Huffman,
    dist: &Huffman,
    max: usize,
) -> Result<(), GzipError> {
    loop {
        let sym = lit.decode(bits)?;
        match sym {
            0..=255 => {
                if out.len() >= max {
                    return Err(GzipError::TooLarge);
                }
                out.push(sym as u8);
            }
            256 => return Ok(()),
            257..=285 => {
                let i = usize::from(sym - 257);
                let len =
                    usize::from(LENGTH_BASE[i]) + bits.take(u32::from(LENGTH_EXTRA[i]))? as usize;
                let d = usize::from(dist.decode(bits)?);
                if d >= 30 {
                    return Err(GzipError::Data);
                }
                let back =
                    usize::from(DIST_BASE[d]) + bits.take(u32::from(DIST_EXTRA[d]))? as usize;
                if back > out.len() {
                    return Err(GzipError::Data);
                }
                if out.len() + len > max {
                    return Err(GzipError::TooLarge);
                }
                let from = out.len() - back;
                for k in 0..len {
                    let b = out[from + k];
                    out.push(b);
                }
            }
            _ => return Err(GzipError::Data),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_the_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn stored_streams_round_trip() {
        for len in [0usize, 1, 100, 0xFFFF, 0x1_0000, 200_000] {
            let data: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8).collect();
            let gz = encode(&data);
            assert_eq!(decode(&gz, usize::MAX), Ok(data));
        }
    }

    #[test]
    fn reads_a_fixed_huffman_stream() {
        // Python's `gzip.compress(b"hello hello hello hello", 9, mtime=0)`: one fixed-Huffman
        // block with a match.
        let gz = [
            0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0xff, 0xcb, 0x48, 0xcd, 0xc9,
            0xc9, 0x57, 0xc8, 0x40, 0x27, 0x01, 0xe3, 0x51, 0x3d, 0x8d, 0x17, 0x00, 0x00, 0x00,
        ];
        assert_eq!(
            decode(&gz, usize::MAX).as_deref(),
            Ok(&b"hello hello hello hello"[..])
        );
    }

    #[test]
    fn reads_a_dynamic_huffman_stream() {
        // Python's `gzip.compress(text, 9, mtime=0)` of 300 random letters and spaces: one
        // dynamic-Huffman block.
        let text = "cbebhhhgdbhaggahedbfaaa agdga dhh dfddheag bcebf g deeh gahdggcf fbh bc gfhahaegcc dad  dg ffhe ag c  dgahf d ghfgfa  fhadc cb eabbahaedebcfebcce ceehfhhbaegfgdebe dgadagcach g d hd agfgaecdaebbeecgeca adhc agdfbdgdhbge hafgeacdfcfgdebg f h dbabccc def efffbedhc bfagbgccfbgb dbefe bhebaeaabgbaddgcbh";
        let gz = [
            0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0xff, 0x1d, 0x8f, 0x41, 0x02,
            0x00, 0x51, 0x04, 0x42, 0xaf, 0xd2, 0xd5, 0x42, 0xe5, 0xfe, 0x27, 0x18, 0x7f, 0x16,
            0x16, 0xa8, 0x3c, 0x5d, 0xaa, 0xdd, 0xcd, 0xd4, 0x32, 0xe1, 0x6a, 0xca, 0x24, 0xc1,
            0x4c, 0x88, 0xd9, 0xc5, 0x78, 0x66, 0xc5, 0xa0, 0x5a, 0x65, 0x04, 0x23, 0x2d, 0x4e,
            0x3a, 0x49, 0x1b, 0xae, 0xbd, 0x0d, 0xe2, 0xe5, 0x52, 0xe9, 0xc6, 0x70, 0x80, 0x09,
            0xec, 0xd5, 0xe5, 0xa0, 0x5f, 0xc7, 0x35, 0x06, 0x59, 0xc7, 0x04, 0x4e, 0x3c, 0x8d,
            0x2e, 0x88, 0x55, 0xcf, 0x37, 0xaa, 0xf6, 0x55, 0x0b, 0x7d, 0xf1, 0xde, 0xad, 0x0b,
            0x73, 0x6e, 0xae, 0xe7, 0x1e, 0xa6, 0xd9, 0xfb, 0xae, 0x63, 0xe7, 0x52, 0x1d, 0xaa,
            0x87, 0xaa, 0x92, 0x3a, 0xea, 0x23, 0x9e, 0xed, 0x87, 0xed, 0x9a, 0xcc, 0x56, 0x84,
            0xa5, 0x23, 0xf6, 0xb8, 0xff, 0xa0, 0x23, 0xc2, 0xbd, 0x53, 0xbc, 0x33, 0x47, 0x29,
            0x43, 0xb6, 0x4b, 0xcf, 0x77, 0x4f, 0xa7, 0x0e, 0xde, 0x95, 0x3a, 0x89, 0x2c, 0xd4,
            0xea, 0x18, 0xc8, 0x9b, 0x70, 0x26, 0x5d, 0xfb, 0x01, 0xfe, 0x6d, 0x69, 0x0a, 0x2c,
            0x01, 0x00, 0x00,
        ];
        assert_eq!(decode(&gz, usize::MAX).as_deref(), Ok(text.as_bytes()));
    }

    #[test]
    fn refuses_what_is_not_gzip_or_too_large() {
        assert_eq!(decode(b"plain", usize::MAX), Err(GzipError::Header));
        let gz = encode(&[7; 100]);
        assert_eq!(decode(&gz, 50), Err(GzipError::TooLarge));
        let mut bad = gz.clone();
        let n = bad.len();
        bad[n - 5] ^= 1;
        assert_eq!(decode(&bad, usize::MAX), Err(GzipError::Check));
    }
}
