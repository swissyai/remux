//! Closed, locally captured PTY corpus manifest and per-step replay traces.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use libghostty_vt::TerminalOptions;

use crate::abi::{AbiImplementation, Operation};
use crate::differential::Pair;
use crate::{fnv1a64, HarnessError, Result};

const CHUNK_BYTES: usize = 4096;
const KINDS: &[&str] = &["vim", "tmux", "less", "top", "curses", "ansi-cat"];

/// One closed provenance row for exact bytes captured from a headless PTY.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CorpusEntry {
    /// Stable corpus ID.
    pub id: String,
    /// Basename under the corpus bytes directory.
    pub file: String,
    /// Closed terminal-program family.
    pub kind: String,
    /// Human-readable command provenance (hex-encoded on disk).
    pub command: String,
    /// Exact byte count pinned by the manifest.
    pub bytes: usize,
    /// FNV-1a integrity digest pinned by the manifest.
    pub digest: u64,
}

/// Aggregate replay result included in the receipt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CorpusSummary {
    /// Captured sessions replayed.
    pub corpora: usize,
    /// Exact captured bytes replayed.
    pub bytes: usize,
    /// Per-chunk state transitions compared.
    pub steps: usize,
}

/// Parses the strict six-column corpus manifest.
///
/// Unknown kinds, duplicate IDs/files, traversal paths, bad lengths, and bad
/// digests fail closed before any bytes are replayed.
pub fn read_manifest(path: &Path) -> Result<Vec<CorpusEntry>> {
    let text = fs::read_to_string(path)?;
    parse_manifest(&text)
}

/// Replays every corpus through an implementation pair and checks each fixed
/// chunk's state-transition digest, never only the final state.
pub fn replay(
    left: &dyn AbiImplementation,
    right: &dyn AbiImplementation,
    corpus_root: &Path,
    trace_root: &Path,
    bless: bool,
) -> Result<CorpusSummary> {
    let entries = read_manifest(&corpus_root.join("manifest.tsv"))?;
    if bless {
        if trace_root.exists() {
            fs::remove_dir_all(trace_root)?;
        }
        fs::create_dir_all(trace_root)?;
    } else if !trace_root.is_dir() {
        return Err(HarnessError::new(format!(
            "corpus trace directory missing: {}",
            trace_root.display()
        )));
    }

    let mut total_bytes = 0_usize;
    let mut total_steps = 0_usize;
    for entry in &entries {
        let bytes_path = corpus_path(corpus_root, &entry.file)?;
        let bytes = fs::read(&bytes_path)?;
        if bytes.len() != entry.bytes {
            return Err(HarnessError::new(format!(
                "corpus {} length {} does not match manifest {}",
                entry.id,
                bytes.len(),
                entry.bytes
            )));
        }
        let digest = fnv1a64(&bytes);
        if digest != entry.digest {
            return Err(HarnessError::new(format!(
                "corpus {} digest {digest:016x} does not match manifest {:016x}",
                entry.id, entry.digest
            )));
        }

        let mut pair = Pair::new(
            left,
            right,
            TerminalOptions {
                cols: 80,
                rows: 24,
                max_scrollback: 512,
            },
        )?;
        let mut previous: Option<Vec<u8>> = None;
        let mut trace = String::from("VT-STEP-TRACE-v2\n");
        for (step, chunk) in bytes.chunks(CHUNK_BYTES).enumerate() {
            let state = pair.apply(
                &Operation::Write(chunk.to_vec()),
                &format!("corpus:{}", entry.id),
            )?;
            let prior = previous.as_deref().unwrap_or_default();
            let (prefix, suffix) = common_edges(prior, &state);
            let removed = prior.len() - prefix - suffix;
            let inserted = &state[prefix..state.len() - suffix];
            let offset = ((step + 1) * CHUNK_BYTES).min(bytes.len());
            trace.push_str(&format!(
                "{step}\t{offset}\t{prefix}\t{suffix}\t{removed}\t{}\n",
                crate::hex(inserted)
            ));
            previous = Some(state);
            total_steps += 1;
        }

        let trace_path = trace_root.join(format!("{}.steps", entry.id));
        if bless {
            fs::write(&trace_path, trace)?;
        } else {
            let expected = fs::read_to_string(&trace_path).map_err(|error| {
                HarnessError::new(format!("read trace {}: {error}", trace_path.display()))
            })?;
            validate_trace(&expected)?;
            if expected != trace {
                return Err(HarnessError::new(format!(
                    "corpus step divergence id={} file={} {}",
                    entry.id,
                    trace_path.display(),
                    first_trace_difference(&expected, &trace)
                )));
            }
        }
        total_bytes = total_bytes
            .checked_add(bytes.len())
            .ok_or_else(|| HarnessError::new("corpus byte total overflow"))?;
    }

    Ok(CorpusSummary {
        corpora: entries.len(),
        bytes: total_bytes,
        steps: total_steps,
    })
}

fn parse_manifest(text: &str) -> Result<Vec<CorpusEntry>> {
    let mut lines = text.lines();
    if lines.next() != Some("VT-CORPUS-v1") {
        return Err(HarnessError::new(
            "corpus manifest header must be VT-CORPUS-v1",
        ));
    }
    let mut ids = BTreeSet::new();
    let mut files = BTreeSet::new();
    let mut entries = Vec::new();
    for (line_index, line) in lines.enumerate() {
        if line.is_empty() {
            return Err(HarnessError::new(format!(
                "empty corpus manifest row {}",
                line_index + 2
            )));
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() != 6 {
            return Err(HarnessError::new(format!(
                "corpus manifest row {} has {} fields, expected 6",
                line_index + 2,
                fields.len()
            )));
        }
        let [id, file, kind, command_hex, bytes_text, digest_text] =
            <[&str; 6]>::try_from(fields.as_slice()).expect("length checked");
        if !valid_id(id) || !valid_file(file) {
            return Err(HarnessError::new(format!(
                "corpus manifest row {} has unsafe id or file",
                line_index + 2
            )));
        }
        if !KINDS.contains(&kind) {
            return Err(HarnessError::new(format!(
                "corpus manifest row {} has unknown kind {kind}",
                line_index + 2
            )));
        }
        if !ids.insert(id.to_owned()) || !files.insert(file.to_owned()) {
            return Err(HarnessError::new(format!(
                "corpus manifest row {} duplicates id or file",
                line_index + 2
            )));
        }
        let command_bytes = decode_hex(command_hex).map_err(|error| {
            HarnessError::new(format!("corpus manifest row {}: {error}", line_index + 2))
        })?;
        let command = String::from_utf8(command_bytes).map_err(|_| {
            HarnessError::new(format!(
                "corpus manifest row {} command is not UTF-8",
                line_index + 2
            ))
        })?;
        if command.is_empty() || command.contains(['\n', '\r', '\0']) {
            return Err(HarnessError::new(format!(
                "corpus manifest row {} command is empty or contains controls",
                line_index + 2
            )));
        }
        let bytes = bytes_text.parse::<usize>().map_err(|_| {
            HarnessError::new(format!(
                "corpus manifest row {} has invalid byte count",
                line_index + 2
            ))
        })?;
        if bytes == 0 {
            return Err(HarnessError::new(format!(
                "corpus manifest row {} is empty",
                line_index + 2
            )));
        }
        if digest_text.len() != 16 || !digest_text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(HarnessError::new(format!(
                "corpus manifest row {} has invalid digest",
                line_index + 2
            )));
        }
        let digest = u64::from_str_radix(digest_text, 16).map_err(|_| {
            HarnessError::new(format!(
                "corpus manifest row {} has invalid digest",
                line_index + 2
            ))
        })?;
        entries.push(CorpusEntry {
            id: id.to_owned(),
            file: file.to_owned(),
            kind: kind.to_owned(),
            command,
            bytes,
            digest,
        });
    }
    if entries.is_empty() {
        return Err(HarnessError::new("corpus manifest has no rows"));
    }
    Ok(entries)
}

fn corpus_path(root: &Path, file: &str) -> Result<PathBuf> {
    if !valid_file(file) {
        return Err(HarnessError::new("unsafe corpus filename"));
    }
    Ok(root.join("bytes").join(file))
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn valid_file(value: &str) -> bool {
    valid_id(value.strip_suffix(".bin").unwrap_or(""))
}

fn decode_hex(value: &str) -> Result<Vec<u8>> {
    if !value.len().is_multiple_of(2) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(HarnessError::new("invalid command hex"));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).expect("ASCII validated");
            u8::from_str_radix(text, 16).map_err(|_| HarnessError::new("invalid command hex"))
        })
        .collect()
}

fn common_edges(previous: &[u8], current: &[u8]) -> (usize, usize) {
    let prefix = previous
        .iter()
        .zip(current)
        .take_while(|(left, right)| left == right)
        .count();
    let maximum_suffix = (previous.len() - prefix).min(current.len() - prefix);
    let suffix = previous
        .iter()
        .rev()
        .zip(current.iter().rev())
        .take(maximum_suffix)
        .take_while(|(left, right)| left == right)
        .count();
    (prefix, suffix)
}

fn validate_trace(text: &str) -> Result<()> {
    let mut lines = text.lines();
    if lines.next() != Some("VT-STEP-TRACE-v2") {
        return Err(HarnessError::new("step trace has invalid header"));
    }
    let mut rows = 0_usize;
    for (index, line) in lines.enumerate() {
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() != 6
            || fields[0].parse::<usize>() != Ok(index)
            || fields[1].parse::<usize>().is_err()
            || fields[2].parse::<usize>().is_err()
            || fields[3].parse::<usize>().is_err()
            || fields[4].parse::<usize>().is_err()
            || !fields[5].len().is_multiple_of(2)
            || !fields[5].bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(HarnessError::new(format!(
                "invalid step trace row {}",
                index + 2
            )));
        }
        rows += 1;
    }
    if rows == 0 {
        return Err(HarnessError::new("step trace has no rows"));
    }
    Ok(())
}

fn first_trace_difference(expected: &str, actual: &str) -> String {
    for (index, (left, right)) in expected.lines().zip(actual.lines()).enumerate() {
        if left != right {
            return format!("line={} expected={left:?} actual={right:?}", index + 1);
        }
    }
    format!(
        "line-count expected={} actual={}",
        expected.lines().count(),
        actual.lines().count()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> String {
        "one\tone.bin\tvim\t76696d\t3\t0123456789abcdef".to_owned()
    }

    #[test]
    fn manifest_parser_accepts_one_closed_row() {
        let parsed = parse_manifest(&format!("VT-CORPUS-v1\n{}\n", row())).expect("manifest");
        assert_eq!(parsed[0].command, "vim");
        assert_eq!(parsed[0].bytes, 3);
    }

    #[test]
    fn manifest_parser_rejects_traversal_duplicate_unknown_and_bad_hex() {
        for bad in [
            "VT-CORPUS-v1\na\t../a.bin\tvim\t76696d\t3\t0123456789abcdef\n",
            "VT-CORPUS-v1\na\ta.bin\tbogus\t76696d\t3\t0123456789abcdef\n",
            "VT-CORPUS-v1\na\ta.bin\tvim\tx\t3\t0123456789abcdef\n",
            "VT-CORPUS-v1\na\ta.bin\tvim\t76696d\t3\t0123456789abcdef\na\tb.bin\tvim\t76696d\t3\t0123456789abcdef\n",
        ] {
            assert!(parse_manifest(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn trace_parser_rejects_bad_header_gap_counts_and_hex() {
        for bad in [
            "wrong\n0\t1\t0\t0\t0\t41\n",
            "VT-STEP-TRACE-v2\n1\t1\t0\t0\t0\t41\n",
            "VT-STEP-TRACE-v2\n0\t1\tx\t0\t0\t41\n",
            "VT-STEP-TRACE-v2\n0\t1\t0\t0\t0\txyz\n",
            "VT-STEP-TRACE-v2\n0\t1\t0\t0\t41\n",
            "VT-STEP-TRACE-v2\n",
        ] {
            assert!(validate_trace(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn exact_diff_edges_never_overlap() {
        assert_eq!(common_edges(b"abcdef", b"abXYef"), (2, 2));
        assert_eq!(common_edges(b"same", b"same"), (4, 0));
        assert_eq!(common_edges(b"", b"new"), (0, 0));
    }
}
