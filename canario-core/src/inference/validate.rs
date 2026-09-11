//! Pre-flight validation of ASR model files, before sherpa-onnx sees them.
//!
//! sherpa-onnx is not defensive at load time: its C++ `ReadTokens`
//! (symbol-table.cc, pinned v1.12.38) calls SHERPA_ONNX_EXIT on input
//! outside its line grammar, killing the *whole app* instead of
//! surfacing an error. Every path that builds a recognizer (dictation
//! via `with_recognizer`, background prewarm via `prewarm_once`, CLI
//! file transcription via `TranscriptionEngine::load_model`) validates
//! the files here first, so a bad tokens file becomes a regular error
//! instead of a process exit.
//!
//! Scope/limitations: the tokens check mirrors sherpa's exact line
//! grammar, so anything that passes it cannot trip SHERPA_ONNX_EXIT in
//! `ReadTokens`. The ONNX check is only a *structural* sanity check
//! (top-level protobuf wire format) — it catches garbage bytes, text
//! files and truncated downloads, but a wire-valid file that is
//! semantically broken can still fail inside ORT. ORT failures normally
//! come back as `OfflineRecognizer::create` returning None (a normal
//! error for us); no pure-Rust pre-check can rule out every possible
//! C++ CHECK/abort in a third-party runtime.

use crate::config::ModelPaths;

/// Hard cap on tokens file size. Real sherpa vocabularies are tens of
/// KB; without a cap, a multi-GB garbage "tokens" file would be read
/// fully into memory before failing the UTF-8 check.
const MAX_TOKENS_BYTES: u64 = 64 * 1024 * 1024;

/// Validate all four model files before sherpa sees them. The tokens
/// check exactly matches sherpa's exit-on-bad-input grammar; the ONNX
/// check is a structural sanity check only (see `validate_onnx`).
/// Returns a human-readable reason on failure (surfaced to the user at
/// dictation/transcription time).
pub(crate) fn validate_model_files(paths: &ModelPaths) -> Result<(), String> {
    validate_tokens(&paths.tokens)?;
    for onnx in [&paths.encoder, &paths.decoder, &paths.joiner] {
        validate_onnx(onnx)?;
    }
    Ok(())
}

/// The tokens file must be UTF-8 text matching the exact line grammar
/// of sherpa's C++ `ReadTokens` (symbol-table.cc, v1.12.38):
///
/// ```text
/// <symbol> <id>   — symbol, ASCII whitespace, int32 id
/// <id>            — a lone integer: the whitespace token " " with that id
/// (blank)         — tolerated by sherpa, skipped here
/// ```
///
/// `ReadTokens` calls SHERPA_ONNX_EXIT on anything outside that grammar:
/// an unparseable or partially-parseable id (`abc`, `12abc`), or extra
/// trailing fields (`symbol extra 1`). Fields are split on ASCII
/// whitespace only, like C++ `operator>>` in the classic locale —
/// Unicode whitespace (e.g. U+00A0) does NOT separate fields for
/// sherpa, so splitting on it here could accept a line sherpa exits on.
///
/// One deliberately stricter case: a lone NON-numeric field. sherpa
/// `atoi`s it to id 0 and loads the (junk) model; we reject it, since
/// no real vocabulary has such lines and the model would be useless.
fn validate_tokens(tokens: &std::path::Path) -> Result<(), String> {
    let name = || tokens.display().to_string();
    let len = std::fs::metadata(tokens)
        .map_err(|e| format!("{}: cannot stat ({})", name(), e))?
        .len();
    if len == 0 {
        return Err(format!("{}: file is empty", name()));
    }
    if len > MAX_TOKENS_BYTES {
        return Err(format!(
            "{}: {} bytes is far too large for a tokens file",
            name(),
            len
        ));
    }
    let text =
        std::fs::read_to_string(tokens).map_err(|_| format!("{}: not valid UTF-8 text", name()))?;
    let mut any_token = false;
    for (i, line) in text.lines().enumerate() {
        // C++ classic-locale whitespace includes vertical tab, which
        // Rust's split_ascii_whitespace deliberately excludes.
        let mut fields = line
            .split(|c: char| c.is_ascii_whitespace() || c == '\u{b}')
            .filter(|field| !field.is_empty());
        let Some(symbol) = fields.next() else {
            continue; // blank line: tolerated by sherpa
        };
        any_token = true;
        match fields.next() {
            // Lone field: the whitespace token, id = the field itself.
            None => {
                if symbol.parse::<i32>().is_err() {
                    return Err(format!(
                        "{}: line {} is a lone non-numeric field",
                        name(),
                        i + 1
                    ));
                }
            }
            // "symbol id": the id must parse FULLY as int32 (sherpa
            // exits on `12abc`), and a third field exits it outright.
            Some(id) => {
                if id.parse::<i32>().is_err() {
                    return Err(format!(
                        "{}: line {} has no parseable integer id",
                        name(),
                        i + 1
                    ));
                }
                if fields.next().is_some() {
                    return Err(format!(
                        "{}: line {} has extra trailing fields",
                        name(),
                        i + 1
                    ));
                }
            }
        }
    }
    if !any_token {
        return Err(format!("{}: contains no tokens", name()));
    }
    Ok(())
}

/// The ONNX encoder/decoder/joiner must look like a protobuf-encoded
/// `ModelProto`. ONNX has **no fixed magic bytes** — a `.onnx` file is
/// just serialized protobuf — so this walks the protobuf wire format
/// instead: every top-level field must have a legal tag and stay within
/// the file, and field 1 (`ir_version`, a `required` varint in
/// onnx.proto) must be present. That catches garbage bytes, text files
/// and truncated downloads without claiming a magic header exists.
///
/// This is a structural sanity check, not a proof of loadability:
/// nested messages are skipped unparsed, so a wire-valid but
/// semantically broken model can still fail inside ORT (normally as
/// `OfflineRecognizer::create` returning None — a regular error for
/// us, since sherpa surfaces ORT failures rather than exiting).
///
/// Reads only tags and lengths (skipping field payloads with `seek`),
/// so even a multi-hundred-MB model validates without being read.
fn validate_onnx(path: &std::path::Path) -> Result<(), String> {
    use std::io::{BufReader, Seek, SeekFrom};

    let name = || path.display().to_string();
    let file = std::fs::File::open(path).map_err(|e| format!("{}: cannot open ({})", name(), e))?;
    let file_len = file
        .metadata()
        .map_err(|e| format!("{}: cannot stat ({})", name(), e))?
        .len();
    if file_len == 0 {
        return Err(format!("{}: file is empty", name()));
    }

    let mut reader = BufReader::new(file);
    let mut saw_ir_version = false;
    loop {
        let pos = reader
            .stream_position()
            .map_err(|e| format!("{}: read error ({})", name(), e))?;
        if pos >= file_len {
            break;
        }
        let key = read_protobuf_varint(&mut reader)
            .ok_or_else(|| format!("{}: truncated or invalid protobuf tag", name()))?;
        let field = key >> 3;
        let wire_type = key & 0x7;
        if field == 0 {
            return Err(format!("{}: invalid protobuf field tag 0", name()));
        }
        match wire_type {
            // Varint
            0 => {
                read_protobuf_varint(&mut reader)
                    .ok_or_else(|| format!("{}: truncated varint field", name()))?;
                if field == 1 {
                    saw_ir_version = true;
                }
            }
            // 64-bit / 32-bit fixed. Bounds are measured AFTER the
            // (possibly multi-byte) tag varint — `pos` predates it.
            1 | 5 => {
                let bytes: u64 = if wire_type == 1 { 8 } else { 4 };
                let after_tag = reader
                    .stream_position()
                    .map_err(|e| format!("{}: read error ({})", name(), e))?;
                if bytes > file_len - after_tag {
                    return Err(format!("{}: truncated fixed-width field", name()));
                }
                reader
                    .seek(SeekFrom::Current(bytes as i64))
                    .map_err(|e| format!("{}: read error ({})", name(), e))?;
            }
            // Length-delimited (strings, nested messages, raw tensors)
            2 => {
                let len = read_protobuf_varint(&mut reader)
                    .ok_or_else(|| format!("{}: truncated length prefix", name()))?;
                let after_len = reader
                    .stream_position()
                    .map_err(|e| format!("{}: read error ({})", name(), e))?;
                if len > file_len - after_len {
                    return Err(format!(
                        "{}: field declares {} bytes past end of file (truncated?)",
                        name(),
                        len
                    ));
                }
                reader
                    .seek(SeekFrom::Current(len as i64))
                    .map_err(|e| format!("{}: read error ({})", name(), e))?;
            }
            // Groups (deprecated) and reserved wire types never appear
            // in ONNX models.
            _ => return Err(format!("{}: unsupported protobuf wire type", name())),
        }
    }
    if !saw_ir_version {
        return Err(format!(
            "{}: no ir_version field — not an ONNX ModelProto",
            name()
        ));
    }
    Ok(())
}

/// Read one protobuf varint (up to 10 bytes for u64). Returns `None`
/// on truncation or overflow.
fn read_protobuf_varint(reader: &mut impl std::io::Read) -> Option<u64> {
    let mut value: u64 = 0;
    let mut buf = [0u8; 1];
    for shift in (0..=63).step_by(7) {
        reader.read_exact(&mut buf).ok()?;
        let byte = buf[0];
        if shift == 63 {
            // 10th byte: only one value bit fits in a u64; a set
            // continuation bit or higher value bits mean overflow.
            if byte > 1 {
                return None;
            }
            value |= (byte as u64) << shift;
        } else {
            value |= ((byte & 0x7f) as u64) << shift;
        }
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// Shared fixtures for validation-related tests across the crate
/// (`recording` builds recognizers through the same guard).
#[cfg(test)]
pub(crate) mod test_support {
    use crate::config::ModelPaths;

    /// Smallest byte string that passes `validate_onnx`: a single
    /// top-level varint field 1 (`ir_version`). ONNX is protobuf with
    /// no magic bytes, so this is all the check can require.
    pub(crate) const MINIMAL_ONNX: &[u8] = &[0x08, 0x01];

    /// Four model file paths inside `dir` (files NOT created).
    pub(crate) fn model_paths_in(dir: &std::path::Path) -> ModelPaths {
        ModelPaths {
            encoder: dir.join("encoder.int8.onnx"),
            decoder: dir.join("decoder.int8.onnx"),
            joiner: dir.join("joiner.int8.onnx"),
            tokens: dir.join("tokens.txt"),
        }
    }

    /// Create all four model files with the given bytes.
    pub(crate) fn write_model_files(paths: &ModelPaths, bytes: &[u8]) {
        for f in [&paths.encoder, &paths.decoder, &paths.joiner, &paths.tokens] {
            std::fs::write(f, bytes).unwrap();
        }
    }

    /// Model files whose tokens are fine but whose ONNX files are
    /// garbage (so nothing ever reaches sherpa).
    pub(crate) fn write_garbage_onnx_model(paths: &ModelPaths) {
        for f in [&paths.encoder, &paths.decoder, &paths.joiner] {
            std::fs::write(f, b"this is not a protobuf message").unwrap();
        }
        std::fs::write(&paths.tokens, "▁t 0\n▁h 1\n").unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    /// The tokens gate is what keeps sherpa's exit()-on-bad-input
    /// `ReadTokens` from ever seeing these files.
    #[test]
    fn validate_tokens_rejects_missing_empty_and_binary() {
        let dir = tempfile::tempdir().unwrap();

        let missing = dir.path().join("missing.txt");
        assert!(validate_tokens(&missing).is_err());

        let empty = dir.path().join("empty.txt");
        std::fs::write(&empty, b"").unwrap();
        assert!(validate_tokens(&empty).is_err());

        // Only blank lines: no tokens at all.
        let blank = dir.path().join("blank.txt");
        std::fs::write(&blank, "\n  \n\t\n").unwrap();
        assert!(validate_tokens(&blank).is_err());

        // Invalid UTF-8 (a lead byte is missing from this emoji).
        let binary = dir.path().join("binary.txt");
        std::fs::write(&binary, [0u8, 159, 146, 150]).unwrap();
        assert!(validate_tokens(&binary).is_err());

        let valid = dir.path().join("tokens.txt");
        std::fs::write(&valid, "▁t 0\n▁h 1\n").unwrap();
        assert!(validate_tokens(&valid).is_ok());
    }

    /// The tokens gate mirrors sherpa's `ReadTokens` line grammar
    /// (symbol-table.cc v1.12.38), because lines outside it make the
    /// C++ code SHERPA_ONNX_EXIT the whole process.
    #[test]
    fn validate_tokens_requires_symbol_id_lines() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, content: &str| {
            let p = dir.path().join(name);
            std::fs::write(&p, content).unwrap();
            p
        };

        // Non-integer id: `iss >> id` fails, trailing data → exit.
        assert!(validate_tokens(&write("bad_id.txt", "▁t abc\n")).is_err());
        // Partially-parseable id: C++ reads 12, then 'a' is trailing
        // data → exit.
        assert!(validate_tokens(&write("partial_id.txt", "▁t 12abc\n")).is_err());
        // Extra trailing fields — even with a parseable LAST field —
        // exit sherpa (`iss >> std::ws` leaves `extra`/`1` unread).
        // Reading only the last field would falsely accept these.
        assert!(validate_tokens(&write("extra_mid.txt", "▁t extra 1\n")).is_err());
        assert!(validate_tokens(&write("vertical_tab_extra.txt", "▁t\u{b}extra 1\n")).is_err());
        assert!(validate_tokens(&write("extra_end.txt", "▁t 1 extra\n")).is_err());
        // Lone non-numeric field: sherpa atoi()s it to 0 and loads a
        // junk model; we reject (stricter, but no real vocab has this).
        assert!(validate_tokens(&write("lone_junk.txt", "hello\n")).is_err());
        // Unicode whitespace does NOT separate fields for C++: the
        // "id" here is `1\u{A0}` — unparseable, sherpa exits.
        assert!(validate_tokens(&write("nbsp.txt", "▁t 1\u{A0}\n")).is_err());

        // ASCII whitespace variants (tab, CRLF, leading space) are fine.
        assert!(validate_tokens(&write("tabs.txt", "▁t\t5\n")).is_ok());
        assert!(validate_tokens(&write("vertical_tab.txt", "▁t\u{b}5\n")).is_ok());
        assert!(validate_tokens(&write("crlf.txt", "  ▁t 5\r\n")).is_ok());
        // Negative ids parse as int32 and sherpa loads them without
        // exiting — accepted (the guard targets exit-causing input).
        assert!(validate_tokens(&write("neg_id.txt", "▁t -1\n")).is_ok());
        // Lone integer = the whitespace token with that id.
        assert!(validate_tokens(&write("space_tok.txt", "▁t 0\n5\n")).is_ok());
        // Multi-blank-line padding is tolerated.
        assert!(validate_tokens(&write("ok.txt", "<blk> 0\n\n▁world 2\n")).is_ok());
    }

    /// A garbage "tokens" file the size of a model must be rejected
    /// from its metadata alone — never read into memory.
    #[test]
    fn validate_tokens_rejects_oversized_files_without_reading() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tokens.txt");
        let f = std::fs::File::create(&path).unwrap();
        f.set_len(MAX_TOKENS_BYTES + 1).unwrap();
        let err = validate_tokens(&path).unwrap_err();
        assert!(err.contains("too large"), "error: {}", err);
    }

    /// ONNX is protobuf with no magic bytes: acceptance hinges on a
    /// well-formed top-level wire stream containing field 1
    /// (`ir_version`), not on any fixed header.
    #[test]
    fn validate_onnx_accepts_well_formed_protobuf() {
        let dir = tempfile::tempdir().unwrap();

        // Just ir_version = 1.
        let minimal = dir.path().join("minimal.onnx");
        std::fs::write(&minimal, MINIMAL_ONNX).unwrap();
        assert!(validate_onnx(&minimal).is_ok());

        // ir_version, then a length-delimited field 7 (graph) whose
        // payload is skipped without parsing, then a varint field 14.
        let with_graph = dir.path().join("graph.onnx");
        std::fs::write(
            &with_graph,
            [0x08, 0x07, 0x3A, 0x03, 0xAA, 0xBB, 0xCC, 0x70, 0x2A],
        )
        .unwrap();
        assert!(validate_onnx(&with_graph).is_ok());

        // ir_version plus fixed32/fixed64 fields (legal wire types),
        // including a multi-byte tag (field 16 ≥ 16 → 2-byte varint).
        let fixed = dir.path().join("fixed.onnx");
        std::fs::write(
            &fixed,
            [
                [0x08, 0x01].as_slice(),               // field 1 varint
                &[0x11, 1, 2, 3, 4, 5, 6, 7, 8],       // field 2, 64-bit
                &[0x1D, 1, 2, 3, 4],                   // field 3, 32-bit
                &[0x81, 0x01, 1, 2, 3, 4, 5, 6, 7, 8], // field 16, 64-bit
            ]
            .concat(),
        )
        .unwrap();
        assert!(validate_onnx(&fixed).is_ok());
    }

    /// Regression: fixed-width bounds must be measured AFTER the tag
    /// varint. With a multi-byte tag (field number ≥ 16), measuring
    /// from the pre-tag position lets a payload run past EOF
    /// undetected (the seek beyond end-of-file succeeds silently).
    #[test]
    fn validate_onnx_rejects_fixed_width_truncated_by_multibyte_tag() {
        let dir = tempfile::tempdir().unwrap();
        // ir_version=1, then field 16 (tag = (16<<3)|1 = 129, varint
        // 0x81 0x01) wire type 1 — only 6 of the 8 payload bytes present.
        let path = dir.path().join("truncated-fixed.onnx");
        std::fs::write(&path, [0x08, 0x01, 0x81, 0x01, 1, 2, 3, 4, 5, 6]).unwrap();
        let err = validate_onnx(&path).unwrap_err();
        assert!(err.contains("truncated"), "error: {}", err);
    }

    /// The rejection cases: everything that must fail BEFORE sherpa or
    /// ORT can see it.
    #[test]
    fn validate_onnx_rejects_garbage_truncated_and_non_model_files() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, content: &[u8]| {
            let p = dir.path().join(name);
            std::fs::write(&p, content).unwrap();
            p
        };

        assert!(validate_onnx(&dir.path().join("missing.onnx")).is_err());
        assert!(validate_onnx(&write("empty.onnx", b"")).is_err());
        // Valid UTF-8 text (passes the tokens check!) is not a model.
        assert!(validate_onnx(&write("text.onnx", b"hello world\n")).is_err());
        // Well-formed protobuf, but no ir_version → not a ModelProto.
        assert!(validate_onnx(&write("no_ir.onnx", &[0x12, 0x01, 0x00])).is_err());
        // Field tag 0 is illegal protobuf.
        assert!(validate_onnx(&write("tag0.onnx", &[0x00, 0x01])).is_err());
        // Deprecated group wire types never appear in ONNX.
        assert!(validate_onnx(&write("group.onnx", &[0x0B, 0x01])).is_err());
        // ir_version, then a field claiming 16 payload bytes with only
        // 1 left — a truncated download.
        assert!(validate_onnx(&write("truncated.onnx", &[0x08, 0x01, 0x3A, 0x10, 0x00])).is_err());
        // Tag varint cut off mid-continuation.
        assert!(validate_onnx(&write("bad_tag.onnx", &[0x88])).is_err());
    }

    /// Regression guard for the process-exit repro: the exact bytes
    /// that killed the test binary with status 255 must now fail
    /// validation on every model file role they could occupy.
    #[test]
    fn validate_model_files_rejects_the_status_255_repro_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let paths = model_paths_in(dir.path());
        write_model_files(&paths, b"\xff\xfe\x00definitely not text");
        let err = validate_model_files(&paths).unwrap_err();
        assert!(err.contains("tokens.txt"), "error: {}", err);

        // Garbage ONNX is caught even with valid tokens.
        write_garbage_onnx_model(&paths);
        assert!(validate_model_files(&paths).is_err());

        // And a fully plausible set passes: real tokens + minimal
        // protobuf-shaped ONNX files.
        for f in [&paths.encoder, &paths.decoder, &paths.joiner] {
            std::fs::write(f, MINIMAL_ONNX).unwrap();
        }
        std::fs::write(&paths.tokens, "▁t 0\n").unwrap();
        assert!(validate_model_files(&paths).is_ok());
    }
}
