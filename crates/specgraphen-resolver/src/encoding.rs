//! Encoding-aware source file reading.
//!
//! Legacy Japanese Java codebases are commonly Shift_JIS / Windows-31J, so
//! reading sources as UTF-8 drops most files. LSP requires UTF-8 text, so we
//! decode here: an explicit encoding wins, otherwise detect by trial.

use anyhow::Result;
use encoding_rs::Encoding;

#[derive(Debug)]
pub struct DecodedSource {
    pub text: String,
    /// Name of the encoding the text was decoded from (e.g. "Shift_JIS").
    pub encoding: &'static str,
    /// True when undecodable bytes were replaced with U+FFFD.
    pub lossy: bool,
}

/// Resolve a user-supplied encoding label (e.g. `shift_jis`, `windows-31j`,
/// `euc-jp`) to an encoding. Labels follow the WHATWG Encoding Standard.
pub fn resolve_encoding(label: &str) -> Result<&'static Encoding> {
    Encoding::for_label(label.trim().as_bytes()).ok_or_else(|| {
        anyhow::anyhow!("Unknown source encoding '{label}' (try e.g. shift_jis, euc-jp, utf-8)")
    })
}

/// Decode source bytes to UTF-8 text. Never fails: when nothing decodes
/// cleanly, falls back to lossy UTF-8 so the file is still usable.
pub fn decode_source(bytes: &[u8], forced: Option<&'static Encoding>) -> DecodedSource {
    if let Some(encoding) = forced {
        let (text, actual, lossy) = encoding.decode(bytes);
        return DecodedSource {
            text: text.into_owned(),
            encoding: actual.name(),
            lossy,
        };
    }

    if let Ok(text) = std::str::from_utf8(bytes) {
        return DecodedSource {
            text: text.to_string(),
            encoding: "UTF-8",
            lossy: false,
        };
    }

    // Trial order matters: Shift_JIS is the most common legacy Japanese
    // encoding, and EUC-JP rarely decodes cleanly as Shift_JIS (and vice versa).
    for encoding in [encoding_rs::SHIFT_JIS, encoding_rs::EUC_JP] {
        let (text, _, had_errors) = encoding.decode(bytes);
        if !had_errors {
            return DecodedSource {
                text: text.into_owned(),
                encoding: encoding.name(),
                lossy: false,
            };
        }
    }

    DecodedSource {
        text: String::from_utf8_lossy(bytes).into_owned(),
        encoding: "UTF-8",
        lossy: true,
    }
}

/// Read a file and decode it to UTF-8 text. Only I/O errors fail.
pub fn read_source(
    path: &std::path::Path,
    forced: Option<&'static Encoding>,
) -> Result<DecodedSource> {
    let bytes = std::fs::read(path)?;
    Ok(decode_source(&bytes, forced))
}

#[cfg(test)]
mod tests {
    use super::*;

    // "日本語" in various encodings
    const SJIS: &[u8] = &[0x93, 0xFA, 0x96, 0x7B, 0x8C, 0xEA];
    const EUC: &[u8] = &[0xC6, 0xFC, 0xCB, 0xDC, 0xB8, 0xEC];

    #[test]
    fn detects_utf8() {
        let d = decode_source("package com.example; // 日本語".as_bytes(), None);
        assert_eq!(d.encoding, "UTF-8");
        assert!(!d.lossy);
    }

    #[test]
    fn detects_shift_jis() {
        let mut bytes = b"// ".to_vec();
        bytes.extend_from_slice(SJIS);
        let d = decode_source(&bytes, None);
        assert_eq!(d.encoding, "Shift_JIS");
        assert_eq!(d.text, "// 日本語");
        assert!(!d.lossy);
    }

    #[test]
    fn detects_euc_jp() {
        let mut bytes = b"// ".to_vec();
        bytes.extend_from_slice(EUC);
        let d = decode_source(&bytes, None);
        assert_eq!(d.encoding, "EUC-JP");
        assert_eq!(d.text, "// 日本語");
    }

    #[test]
    fn forced_encoding_wins_over_detection() {
        // Plain ASCII would detect as UTF-8, but a forced encoding is honored
        let enc = resolve_encoding("windows-31j").unwrap();
        let d = decode_source(b"package com.example;", Some(enc));
        assert_eq!(d.encoding, "Shift_JIS"); // windows-31j is an alias label
        assert_eq!(d.text, "package com.example;");
    }

    #[test]
    fn never_fails_on_garbage() {
        let d = decode_source(&[0xFF, 0xFE, 0x80, 0x80, 0xFF], None);
        assert!(d.lossy);
        assert!(!d.text.is_empty());
    }

    #[test]
    fn rejects_unknown_label() {
        assert!(resolve_encoding("not-a-charset").is_err());
        assert!(resolve_encoding("shift_jis").is_ok());
        assert!(resolve_encoding("euc-jp").is_ok());
    }
}
