//! How long a tune plays.
//!
//! A PSID file carries no duration: most tunes loop forever, and the play
//! routine gives no sign of having reached the end. The High Voltage SID
//! Collection keeps the lengths separately, in `DOCUMENTS/Songlengths.md5`,
//! keyed by the MD5 of the whole file. Lines look like
//!
//! ```text
//! ; /MUSICIANS/H/Hubbard_Rob/Commando.sid
//! 2d5f6c0b9b1a3e6f8c4d7e9a0b1c2d3e=4:24 0:04.500 0:13
//! ```
//!
//! with one `m:ss[.mmm]` entry per song, in song order. Older databases tag an
//! entry with an attribute such as `(G)` or `(M)`; those are ignored.

use std::path::{Path, PathBuf};

/// Where HVSC keeps the database, relative to the collection root.
const HVSC_DATABASE: &str = "DOCUMENTS/Songlengths.md5";

/// The MD5 digest of `data` (RFC 1321), as 32 lowercase hex digits.
pub fn md5_hex(data: &[u8]) -> String {
    md5(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// The MD5 digest of `data` (RFC 1321).
pub fn md5(data: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    // floor(abs(sin(i + 1)) * 2^32), the table RFC 1321 lists.
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];

    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64).wrapping_mul(8)).to_le_bytes());

    let mut h: [u32; 4] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476];
    for chunk in msg.chunks_exact(64) {
        let m: Vec<u32> = chunk
            .chunks_exact(4)
            .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
            .collect();
        let [mut a, mut b, mut c, mut d] = h;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let rotated = a
                .wrapping_add(f)
                .wrapping_add(K[i])
                .wrapping_add(m[g])
                .rotate_left(S[i]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(rotated);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
    }

    let mut out = [0u8; 16];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    out
}

/// One `m:ss`, `m:ss.f` or `m:ss.mmm` entry, in seconds. An attribute suffix
/// such as `(G)` is dropped.
fn parse_time(entry: &str) -> Option<f64> {
    let entry = entry.split('(').next()?.trim();
    let (min, sec) = entry.split_once(':')?;
    let min: u32 = min.parse().ok()?;
    let sec: f64 = sec.parse().ok()?;
    (0.0..60.0)
        .contains(&sec)
        .then(|| f64::from(min) * 60.0 + sec)
}

/// The length of 1-based `song` in a database's text, for the file whose MD5
/// is `md5`. `None` when the file or the song is not listed.
pub fn lookup(database: &str, md5: &str, song: u8) -> Option<f64> {
    let index = usize::from(song).checked_sub(1)?;
    database
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with(';') && !l.starts_with('['))
        .find_map(|l| {
            let (key, times) = l.split_once('=')?;
            key.trim().eq_ignore_ascii_case(md5).then_some(times)
        })
        .and_then(|times| times.split_whitespace().nth(index))
        .and_then(parse_time)
        .filter(|&s| s > 0.0)
}

/// Look for an HVSC database above `tune`: a tune at
/// `C64Music/MUSICIANS/H/Hubbard_Rob/Commando.sid` finds
/// `C64Music/DOCUMENTS/Songlengths.md5`. A `Songlengths.md5` sitting directly in
/// one of those directories is accepted too, for loose collections.
pub fn find_database(tune: &Path) -> Option<PathBuf> {
    let start = std::fs::canonicalize(tune).ok()?;
    start.ancestors().skip(1).find_map(|dir| {
        [dir.join(HVSC_DATABASE), dir.join("Songlengths.md5")]
            .into_iter()
            .find(|p| p.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_matches_the_rfc_1321_test_suite() {
        for (input, digest) in [
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("a", "0cc175b9c0f1b6a831c399e269772661"),
            ("abc", "900150983cd24fb0d6963f7d28e17f72"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            (
                "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ] {
            assert_eq!(md5_hex(input.as_bytes()), digest, "md5({input:?})");
        }
    }

    #[test]
    fn times_parse_with_and_without_fractions_and_attributes() {
        assert_eq!(parse_time("4:24"), Some(264.0));
        assert_eq!(parse_time("0:04.500"), Some(4.5));
        assert_eq!(parse_time("1:02(G)"), Some(62.0));
        assert_eq!(parse_time("1:75"), None);
        assert_eq!(parse_time("abc"), None);
    }

    #[test]
    fn lookup_picks_the_song_by_its_1_based_number() {
        let db = "[Database]\n\
                  ; /MUSICIANS/X/Tune.sid\n\
                  0123456789ABCDEF0123456789abcdef=3:00 0:04.5 0:13\n\
                  ffffffffffffffffffffffffffffffff=1:00\n";
        let md5 = "0123456789abcdef0123456789abcdef";
        assert_eq!(lookup(db, md5, 1), Some(180.0));
        assert_eq!(lookup(db, md5, 2), Some(4.5));
        assert_eq!(lookup(db, md5, 4), None);
        assert_eq!(lookup(db, md5, 0), None);
        assert_eq!(lookup(db, "00000000000000000000000000000000", 1), None);
    }
}
