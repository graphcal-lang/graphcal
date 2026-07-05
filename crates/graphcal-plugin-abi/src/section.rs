//! Reading and writing the manifest custom section of a `.wasm` binary.
//!
//! Implements just enough of the WebAssembly binary format — the 8-byte
//! header and the section framing (`id: u8`, `size: u32` as LEB128, then
//! `size` payload bytes; custom sections are id 0 with a name-prefixed
//! payload) — to locate, extract, and append the
//! [`MANIFEST_SECTION`](crate::MANIFEST_SECTION) custom section without any
//! WASM runtime dependency. Non-custom sections are skipped over by their
//! declared size; their contents are never interpreted.

use thiserror::Error;

/// The 8-byte header every wasm module starts with: magic `\0asm` plus
/// binary format version 1 (little endian).
///
/// Also the smallest valid wasm module — handy as a manifest-embedding
/// target in tests.
pub const EMPTY_MODULE: [u8; 8] = [0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];

/// Section id of a wasm custom section.
const CUSTOM_SECTION_ID: u8 = 0;

/// Extract the payload of the unique manifest custom section from a wasm
/// binary.
///
/// # Errors
///
/// Returns [`SectionError`] when the bytes are not a wasm module, the
/// section framing is malformed, or the module does not contain exactly one
/// manifest section.
pub fn extract_manifest(wasm: &[u8]) -> Result<&[u8], SectionError> {
    let mut manifest: Option<&[u8]> = None;
    let mut walker = SectionWalker::new(wasm)?;
    while let Some(section) = walker.next_section()? {
        if section.name == crate::MANIFEST_SECTION {
            if manifest.is_some() {
                return Err(SectionError::DuplicateManifest);
            }
            manifest = Some(section.payload);
        }
    }
    manifest.ok_or(SectionError::MissingManifest)
}

/// Append `payload` to a wasm binary as the manifest custom section.
///
/// Custom sections may appear at any position between other sections, so
/// appending at the end always yields a spec-valid module.
///
/// # Errors
///
/// Returns [`SectionError`] when the bytes are not a wasm module, the
/// section framing is malformed, the module already contains a manifest
/// section, or the section would exceed the format's `u32` size limit.
pub fn embed_manifest(wasm: &[u8], payload: &[u8]) -> Result<Vec<u8>, SectionError> {
    let mut walker = SectionWalker::new(wasm)?;
    while let Some(section) = walker.next_section()? {
        if section.name == crate::MANIFEST_SECTION {
            return Err(SectionError::DuplicateManifest);
        }
    }

    let name = crate::MANIFEST_SECTION.as_bytes();
    let name_len =
        encode_leb128_u32(u32::try_from(name.len()).map_err(|_| SectionError::TooLarge)?);
    let content_len = name_len
        .len()
        .checked_add(name.len())
        .and_then(|n| n.checked_add(payload.len()))
        .and_then(|n| u32::try_from(n).ok())
        .ok_or(SectionError::TooLarge)?;
    let size = encode_leb128_u32(content_len);

    let mut out = Vec::with_capacity(wasm.len() + 1 + size.len() + content_len as usize);
    out.extend_from_slice(wasm);
    out.push(CUSTOM_SECTION_ID);
    out.extend_from_slice(&size);
    out.extend_from_slice(&name_len);
    out.extend_from_slice(name);
    out.extend_from_slice(payload);
    Ok(out)
}

/// One custom section encountered by the walker.
struct CustomSection<'a> {
    name: &'a str,
    payload: &'a [u8],
}

/// Cursor over a wasm binary's section framing, yielding custom sections.
struct SectionWalker<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> SectionWalker<'a> {
    fn new(wasm: &'a [u8]) -> Result<Self, SectionError> {
        if wasm.len() < EMPTY_MODULE.len() || wasm[..4] != EMPTY_MODULE[..4] {
            return Err(SectionError::NotAWasmModule);
        }
        let version = u32::from_le_bytes([wasm[4], wasm[5], wasm[6], wasm[7]]);
        if version != 1 {
            return Err(SectionError::UnsupportedWasmVersion { found: version });
        }
        Ok(Self {
            bytes: wasm,
            pos: EMPTY_MODULE.len(),
        })
    }

    /// Advance to the next custom section, skipping non-custom sections.
    ///
    /// Returns `Ok(None)` at the end of the module.
    fn next_section(&mut self) -> Result<Option<CustomSection<'a>>, SectionError> {
        while self.pos < self.bytes.len() {
            let id = self.bytes[self.pos];
            self.pos += 1;
            let size = self.read_leb128_u32()? as usize;
            let end = self
                .pos
                .checked_add(size)
                .filter(|end| *end <= self.bytes.len())
                .ok_or(SectionError::Truncated { offset: self.pos })?;
            let body = &self.bytes[self.pos..end];
            self.pos = end;
            if id != CUSTOM_SECTION_ID {
                continue;
            }

            let (name_len, name_len_bytes) = decode_leb128_u32(body)
                .ok_or(SectionError::InvalidSectionName { offset: end - size })?;
            let name_end = name_len_bytes
                .checked_add(name_len as usize)
                .filter(|name_end| *name_end <= body.len())
                .ok_or(SectionError::InvalidSectionName { offset: end - size })?;
            let name = std::str::from_utf8(&body[name_len_bytes..name_end])
                .map_err(|_| SectionError::InvalidSectionName { offset: end - size })?;
            return Ok(Some(CustomSection {
                name,
                payload: &body[name_end..],
            }));
        }
        Ok(None)
    }

    fn read_leb128_u32(&mut self) -> Result<u32, SectionError> {
        let offset = self.pos;
        let (value, consumed) = decode_leb128_u32(&self.bytes[self.pos..])
            .ok_or(SectionError::MalformedLength { offset })?;
        self.pos += consumed;
        Ok(value)
    }
}

/// Decode an LEB128-encoded `u32`, returning the value and the number of
/// bytes consumed. Returns `None` on truncation, on encodings longer than
/// five bytes, or when set bits overflow 32 bits.
fn decode_leb128_u32(bytes: &[u8]) -> Option<(u32, usize)> {
    let mut value: u32 = 0;
    for (index, &byte) in bytes.iter().enumerate().take(5) {
        let bits = u32::from(byte & 0x7F);
        // The final (fifth) byte may only contribute the low 4 bits.
        if index == 4 && byte & 0xF0 != 0 {
            return None;
        }
        value |= bits << (7 * index);
        if byte & 0x80 == 0 {
            return Some((value, index + 1));
        }
    }
    None
}

/// Encode a `u32` as LEB128.
fn encode_leb128_u32(mut value: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(5);
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return out;
        }
        out.push(byte | 0x80);
    }
}

/// Error from reading or extending a wasm binary's section layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum SectionError {
    /// The bytes do not start with the wasm magic header.
    #[error("not a WebAssembly module (bad or truncated magic header)")]
    NotAWasmModule,
    /// The wasm binary format version is not 1.
    #[error("unsupported WebAssembly binary version {found} (expected 1)")]
    UnsupportedWasmVersion {
        /// The version field found in the header.
        found: u32,
    },
    /// A section's declared size runs past the end of the binary.
    #[error("malformed WebAssembly module: section at byte {offset} is truncated")]
    Truncated {
        /// Byte offset of the truncated section's payload.
        offset: usize,
    },
    /// A section size field is not a valid LEB128 `u32`.
    #[error("malformed WebAssembly module: invalid section length at byte {offset}")]
    MalformedLength {
        /// Byte offset of the malformed length field.
        offset: usize,
    },
    /// A custom section's name is malformed (bad length or not UTF-8).
    #[error("malformed WebAssembly module: invalid custom section name at byte {offset}")]
    InvalidSectionName {
        /// Byte offset of the custom section's payload.
        offset: usize,
    },
    /// The module contains no manifest custom section.
    #[error(
        "the WebAssembly module embeds no `{section}` custom section; \
         graphcal plugins must embed their manifest",
        section = crate::MANIFEST_SECTION
    )]
    MissingManifest,
    /// The module contains more than one manifest custom section (or one is
    /// being embedded into a module that already has one).
    #[error(
        "the WebAssembly module contains more than one `{section}` custom section",
        section = crate::MANIFEST_SECTION
    )]
    DuplicateManifest,
    /// The manifest section would exceed the format's `u32` size limit.
    #[error("the plugin manifest is too large to embed as a WebAssembly custom section")]
    TooLarge,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-encode a custom section with the given name and payload.
    fn custom_section(name: &str, payload: &[u8]) -> Vec<u8> {
        let name_len = encode_leb128_u32(u32::try_from(name.len()).unwrap());
        let content_len = u32::try_from(name_len.len() + name.len() + payload.len()).unwrap();
        let mut out = vec![CUSTOM_SECTION_ID];
        out.extend_from_slice(&encode_leb128_u32(content_len));
        out.extend_from_slice(&name_len);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(payload);
        out
    }

    /// A fabricated non-custom section (id 1) with an opaque 3-byte payload.
    fn opaque_type_section() -> Vec<u8> {
        vec![1, 3, 0xAA, 0xBB, 0xCC]
    }

    #[test]
    fn embed_then_extract_roundtrips() {
        let wasm = embed_manifest(&EMPTY_MODULE, b"{\"payload\":1}").unwrap();
        assert_eq!(extract_manifest(&wasm).unwrap(), b"{\"payload\":1}");
    }

    #[test]
    fn skips_other_sections_and_foreign_custom_sections() {
        let mut wasm = EMPTY_MODULE.to_vec();
        wasm.extend_from_slice(&opaque_type_section());
        wasm.extend_from_slice(&custom_section("producers", b"rustc"));
        let wasm = embed_manifest(&wasm, b"manifest-bytes").unwrap();
        assert_eq!(extract_manifest(&wasm).unwrap(), b"manifest-bytes");
    }

    #[test]
    fn empty_payload_roundtrips() {
        let wasm = embed_manifest(&EMPTY_MODULE, b"").unwrap();
        assert_eq!(extract_manifest(&wasm).unwrap(), b"");
    }

    #[test]
    fn missing_manifest_is_reported() {
        assert_eq!(
            extract_manifest(&EMPTY_MODULE).unwrap_err(),
            SectionError::MissingManifest
        );
    }

    #[test]
    fn duplicate_manifest_is_reported_on_extract() {
        let mut wasm = embed_manifest(&EMPTY_MODULE, b"a").unwrap();
        wasm.extend_from_slice(&custom_section(crate::MANIFEST_SECTION, b"b"));
        assert_eq!(
            extract_manifest(&wasm).unwrap_err(),
            SectionError::DuplicateManifest
        );
    }

    #[test]
    fn embedding_twice_is_rejected() {
        let once = embed_manifest(&EMPTY_MODULE, b"a").unwrap();
        assert_eq!(
            embed_manifest(&once, b"b").unwrap_err(),
            SectionError::DuplicateManifest
        );
    }

    #[test]
    fn bad_magic_is_rejected() {
        assert_eq!(
            extract_manifest(b"\0amateur").unwrap_err(),
            SectionError::NotAWasmModule
        );
        assert_eq!(
            extract_manifest(&EMPTY_MODULE[..7]).unwrap_err(),
            SectionError::NotAWasmModule
        );
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let mut wasm = EMPTY_MODULE;
        wasm[4] = 2;
        assert_eq!(
            extract_manifest(&wasm).unwrap_err(),
            SectionError::UnsupportedWasmVersion { found: 2 }
        );
    }

    #[test]
    fn section_running_past_the_end_is_truncated() {
        let mut wasm = EMPTY_MODULE.to_vec();
        wasm.extend_from_slice(&[1, 100, 0xAA]); // declares 100 bytes, has 1
        assert_eq!(
            extract_manifest(&wasm).unwrap_err(),
            SectionError::Truncated { offset: 10 }
        );
    }

    #[test]
    fn oversized_leb128_length_is_malformed() {
        let mut wasm = EMPTY_MODULE.to_vec();
        // Six continuation bytes: longer than any valid LEB128 u32.
        wasm.extend_from_slice(&[1, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01]);
        assert_eq!(
            extract_manifest(&wasm).unwrap_err(),
            SectionError::MalformedLength { offset: 9 }
        );
    }

    #[test]
    fn leb128_bits_beyond_u32_are_malformed() {
        let mut wasm = EMPTY_MODULE.to_vec();
        // Fifth byte contributes bits 28.. — 0x10 sets bit 32.
        wasm.extend_from_slice(&[1, 0x80, 0x80, 0x80, 0x80, 0x10]);
        assert_eq!(
            extract_manifest(&wasm).unwrap_err(),
            SectionError::MalformedLength { offset: 9 }
        );
    }

    #[test]
    fn truncated_leb128_length_is_malformed() {
        let mut wasm = EMPTY_MODULE.to_vec();
        wasm.extend_from_slice(&[1, 0x80]); // continuation bit set, then EOF
        assert_eq!(
            extract_manifest(&wasm).unwrap_err(),
            SectionError::MalformedLength { offset: 9 }
        );
    }

    #[test]
    fn custom_section_name_longer_than_body_is_invalid() {
        let mut wasm = EMPTY_MODULE.to_vec();
        wasm.extend_from_slice(&[0, 2, 200, 0xAA]); // name_len 200 in a 1-byte body
        assert_eq!(
            extract_manifest(&wasm).unwrap_err(),
            SectionError::InvalidSectionName { offset: 10 }
        );
    }

    #[test]
    fn non_utf8_custom_section_name_is_invalid() {
        let mut wasm = EMPTY_MODULE.to_vec();
        wasm.extend_from_slice(&[0, 3, 2, 0xFF, 0xFE]); // 2-byte non-UTF-8 name
        assert_eq!(
            extract_manifest(&wasm).unwrap_err(),
            SectionError::InvalidSectionName { offset: 10 }
        );
    }

    #[test]
    fn leb128_encoding_roundtrips() {
        for value in [0, 1, 127, 128, 300, 0x4000, u32::MAX] {
            let encoded = encode_leb128_u32(value);
            assert_eq!(decode_leb128_u32(&encoded), Some((value, encoded.len())));
        }
    }

    #[test]
    fn non_canonical_leb128_lengths_are_accepted() {
        // 3 encoded as two bytes (0x83 0x00) — legal per the wasm spec.
        let mut wasm = EMPTY_MODULE.to_vec();
        wasm.extend_from_slice(&[1, 0x83, 0x00, 0xAA, 0xBB, 0xCC]);
        let wasm = embed_manifest(&wasm, b"x").unwrap();
        assert_eq!(extract_manifest(&wasm).unwrap(), b"x");
    }
}
