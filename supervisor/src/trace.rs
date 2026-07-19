// Tests prioritize: fast, deterministic, isolated, behavior-sensitive, structure-insensitive, specific, readable, writable, predictive, and inspiring.
//! Strict format for a recorded, then replayed real working-session trace.
//!
//! Contract: trace bytes come from a prior live PTY capture. Files carry source,
//! capture time, command digest, exit status, declared count, monotonic spacing,
//! and hex-encoded logical lines. Parsing is bounded and rejects unknown fields,
//! malformed hex, count differences, and timestamp regressions.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use crate::digest::{digest_parts, hex};

const TRACE_MAGIC: &str = "REMUX_TRACE_V1";
const TRACE_SOURCE: &str = "live-working-session";
const MAX_TRACE_BYTES: usize = 4 * 1024 * 1024;
const MAX_TRACE_RECORDS: usize = 4_096;
const MAX_TRACE_LINE_BYTES: usize = 4_096;

/// One normalized logical PTY line and its offset from capture start.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceRecord {
    at_micros: u64,
    bytes: Vec<u8>,
}

impl TraceRecord {
    /// Constructs one bounded capture record.
    pub fn new(at_micros: u64, bytes: Vec<u8>) -> io::Result<Self> {
        if bytes.is_empty() || bytes.len() > MAX_TRACE_LINE_BYTES {
            return Err(invalid_input("invalid trace line length"));
        }
        Ok(Self { at_micros, bytes })
    }

    /// Monotonic offset from capture start.
    #[must_use]
    pub fn at_micros(&self) -> u64 {
        self.at_micros
    }

    /// Normalized logical line bytes without CR/LF framing.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Complete validated working-session trace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordedTrace {
    captured_unix_micros: u64,
    command_sha256: String,
    exit_code: i32,
    records: Vec<TraceRecord>,
}

impl RecordedTrace {
    /// Builds a trace from live-captured records and hashes the executed command.
    pub fn from_capture(
        captured_unix_micros: u64,
        command: &str,
        exit_code: i32,
        records: Vec<TraceRecord>,
    ) -> io::Result<Self> {
        validate_records(&records)?;
        if exit_code != 0 {
            return Err(invalid_input("working-session capture did not exit zero"));
        }
        Ok(Self {
            captured_unix_micros,
            command_sha256: hex(&digest_parts(&[command.as_bytes()])?),
            exit_code,
            records,
        })
    }

    /// Reads and strictly validates one bounded trace file.
    pub fn read(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let limit = u64::try_from(MAX_TRACE_BYTES.saturating_add(1)).map_err(io::Error::other)?;
        let mut bytes = Vec::new();
        file.take(limit).read_to_end(&mut bytes)?;
        if bytes.len() > MAX_TRACE_BYTES {
            return Err(invalid_data("recorded trace exceeds size limit"));
        }
        let input = String::from_utf8(bytes)
            .map_err(|_| invalid_data("recorded trace is not UTF-8 framing"))?;
        Self::parse(&input)
    }

    /// Atomically writes the complete trace and synchronizes its parent directory.
    pub fn write_atomic(&self, path: &Path) -> io::Result<()> {
        let encoded = self.encode();
        let temporary = temporary_path(path)?;
        let write_result = (|| {
            let mut file = File::create(&temporary)?;
            file.write_all(encoded.as_bytes())?;
            file.sync_all()?;
            fs::rename(&temporary, path)?;
            let parent = path.parent().unwrap_or_else(|| Path::new("."));
            File::open(parent)?.sync_all()
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        write_result
    }

    /// Capture Unix timestamp in microseconds.
    #[must_use]
    pub fn captured_unix_micros(&self) -> u64 {
        self.captured_unix_micros
    }

    /// SHA-256 of the command executed during live capture.
    #[must_use]
    pub fn command_sha256(&self) -> &str {
        &self.command_sha256
    }

    /// Captured process exit code.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        self.exit_code
    }

    /// Captured logical records in monotonic order.
    #[must_use]
    pub fn records(&self) -> &[TraceRecord] {
        &self.records
    }

    fn parse(input: &str) -> io::Result<Self> {
        if !input.ends_with('\n') {
            return Err(invalid_data("recorded trace has a torn final line"));
        }
        let mut lines = input.lines();
        if lines.next() != Some(TRACE_MAGIC) {
            return Err(invalid_data("invalid recorded trace magic"));
        }
        expect_field(&mut lines, "source", TRACE_SOURCE)?;
        let captured_unix_micros = parse_u64(
            field(&mut lines, "captured_unix_micros")?,
            "capture timestamp",
        )?;
        let command_sha256 = field(&mut lines, "command_sha256")?.to_owned();
        if !is_lower_hex_digest(&command_sha256) {
            return Err(invalid_data("invalid trace command digest"));
        }
        let exit_code = field(&mut lines, "exit_code")?
            .parse::<i32>()
            .map_err(|_| invalid_data("invalid trace exit code"))?;
        if exit_code != 0 {
            return Err(invalid_data("recorded working session did not exit zero"));
        }
        let declared_count = usize::try_from(parse_u64(
            field(&mut lines, "record_count")?,
            "record count",
        )?)
        .map_err(|_| invalid_data("trace record count overflow"))?;
        if declared_count == 0 || declared_count > MAX_TRACE_RECORDS {
            return Err(invalid_data("trace record count exceeds bounds"));
        }
        let records = lines.map(parse_record).collect::<io::Result<Vec<_>>>()?;
        if records.len() != declared_count {
            return Err(invalid_data("trace record count differs"));
        }
        validate_records(&records)?;
        Ok(Self {
            captured_unix_micros,
            command_sha256,
            exit_code,
            records,
        })
    }

    fn encode(&self) -> String {
        let mut output = format!(
            "{TRACE_MAGIC}\nsource\t{TRACE_SOURCE}\ncaptured_unix_micros\t{}\ncommand_sha256\t{}\nexit_code\t{}\nrecord_count\t{}\n",
            self.captured_unix_micros,
            self.command_sha256,
            self.exit_code,
            self.records.len()
        );
        for record in &self.records {
            output.push_str(&record.at_micros.to_string());
            output.push('\t');
            output.push_str(&hex(&record.bytes));
            output.push('\n');
        }
        output
    }
}

fn parse_record(line: &str) -> io::Result<TraceRecord> {
    let (at_micros, encoded) = line
        .split_once('\t')
        .ok_or_else(|| invalid_data("invalid trace record shape"))?;
    let at_micros = parse_u64(at_micros, "record timestamp")?;
    let bytes = decode_hex(encoded)?;
    TraceRecord::new(at_micros, bytes).map_err(|error| invalid_data(error.to_string()))
}

fn validate_records(records: &[TraceRecord]) -> io::Result<()> {
    if records.is_empty() || records.len() > MAX_TRACE_RECORDS {
        return Err(invalid_input("trace record count exceeds bounds"));
    }
    let mut previous = None;
    for record in records {
        if record.bytes.is_empty() || record.bytes.len() > MAX_TRACE_LINE_BYTES {
            return Err(invalid_input("invalid trace line length"));
        }
        if previous.is_some_and(|timestamp| record.at_micros < timestamp) {
            return Err(invalid_input("trace record timestamp regressed"));
        }
        previous = Some(record.at_micros);
    }
    Ok(())
}

fn field<'a>(lines: &mut impl Iterator<Item = &'a str>, expected: &str) -> io::Result<&'a str> {
    let line = lines
        .next()
        .ok_or_else(|| invalid_data(format!("missing trace field {expected}")))?;
    let (name, value) = line
        .split_once('\t')
        .ok_or_else(|| invalid_data("invalid trace header field"))?;
    if name != expected || value.is_empty() {
        return Err(invalid_data(format!("invalid trace field {expected}")));
    }
    Ok(value)
}

fn expect_field<'a>(
    lines: &mut impl Iterator<Item = &'a str>,
    expected: &str,
    expected_value: &str,
) -> io::Result<()> {
    if field(lines, expected)? == expected_value {
        Ok(())
    } else {
        Err(invalid_data(format!("invalid trace field {expected}")))
    }
}

fn decode_hex(value: &str) -> io::Result<Vec<u8>> {
    if value.is_empty() || value.len() % 2 != 0 || value.len() / 2 > MAX_TRACE_LINE_BYTES {
        return Err(invalid_data("invalid trace hex length"));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_digit(pair[0])?;
            let low = hex_digit(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_digit(byte: u8) -> io::Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(invalid_data("invalid lowercase trace hex")),
    }
}

fn parse_u64(value: &str, name: &str) -> io::Result<u64> {
    if value.len() > 20 || (value.len() > 1 && value.starts_with('0')) {
        return Err(invalid_data(format!("invalid trace {name}")));
    }
    value
        .parse::<u64>()
        .map_err(|_| invalid_data(format!("invalid trace {name}")))
}

fn is_lower_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn temporary_path(path: &Path) -> io::Result<PathBuf> {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| invalid_input("trace path needs a UTF-8 file name"))?;
    Ok(path.with_file_name(format!(".{name}.tmp-{}", std::process::id())))
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::{RecordedTrace, TraceRecord};

    #[test]
    fn real_trace_format_round_trips_binary_lines_and_timing() {
        let trace = RecordedTrace::from_capture(
            42,
            "cargo test --offline",
            0,
            vec![
                TraceRecord::new(0, b"first".to_vec()).expect("first record"),
                TraceRecord::new(12_345, vec![0, 1, 0xfe, 0xff]).expect("binary record"),
            ],
        )
        .expect("valid trace");
        let encoded = trace.encode();
        let decoded = RecordedTrace::parse(&encoded).expect("strict trace parse");
        assert_eq!(decoded, trace);
    }

    #[test]
    fn malformed_truncated_and_regressing_trace_records_fail_closed() {
        let valid = "REMUX_TRACE_V1\nsource\tlive-working-session\ncaptured_unix_micros\t42\ncommand_sha256\t0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\nexit_code\t0\nrecord_count\t2\n10\t61\n20\t62\n";
        assert!(RecordedTrace::parse(valid).is_ok());
        for malformed in [
            &valid[..valid.len() - 1],
            &valid.replace("record_count\t2", "record_count\t3"),
            &valid.replace("20\t62", "5\t62"),
            &valid.replace("10\t61", "10\t6z"),
            &valid.replace("source\tlive-working-session", "source\tsynthetic"),
        ] {
            assert!(
                RecordedTrace::parse(malformed).is_err(),
                "accepted malformed trace: {malformed:?}"
            );
        }
    }
}
