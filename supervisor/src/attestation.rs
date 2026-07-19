//! Supervisor-owned, append-only session attestation logs.
//!
//! Contract: an [`AttestationObserver`] is minted only from an exact observe
//! capability. It accepts supervisor observations, never agent-authored record
//! fields. A dedicated writer hashes, frames, appends, and synchronizes every
//! record. Verification is std-only and treats mutation as invalid data while
//! distinguishing an incomplete final frame as a recoverable torn tail.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::capability::{observed_sessions, ObserveCapability, ObservedSessions};
use crate::digest::{digest_parts, hex, Sha256};

const FRAME_MAGIC: &[u8; 4] = b"RMA4";
const COMMIT_MAGIC: &[u8; 4] = b"END4";
const CHAIN_DOMAIN: &[u8] = b"remux-attestation-chain-v1\0";
const MAX_PAYLOAD_BYTES: usize = 512;
const MAX_LOG_BYTES: u64 = 64 * 1024 * 1024;
const ZERO_HASH: [u8; 32] = [0; 32];

/// Session lifecycle transition observed by the supervisor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecyclePhase {
    /// Session exists in supervisor state but no child has been observed yet.
    Created,
    /// Authorized child spawn completed.
    Running,
    /// Child exit and all PTY reads completed.
    Ended,
}

impl LifecyclePhase {
    fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Running => "running",
            Self::Ended => "ended",
        }
    }
}

/// Process outcome observed through the supervisor's child handle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExitOutcome {
    /// Normal exit code.
    Code(i32),
    /// Unix terminating signal.
    Signal(i32),
}

impl ExitOutcome {
    fn encode(self) -> String {
        match self {
            Self::Code(code) => format!("code:{code}"),
            Self::Signal(signal) => format!("signal:{signal}"),
        }
    }
}

/// Integrity state of one externally verified file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogIntegrity {
    /// Every byte belongs to a committed, valid hash-chain frame.
    Complete,
    /// A valid prefix is followed by an incomplete final frame.
    TornTail,
}

/// Externally checkable verification result for one session file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Verification {
    /// Whether the file is complete or has one torn final suffix.
    pub integrity: LogIntegrity,
    /// Number of complete records in the valid prefix.
    pub records: u64,
    /// Hash-chain head after the complete prefix.
    pub head: [u8; 32],
    /// Cumulative supervisor-observed PTY input bytes.
    pub input_bytes: u64,
    /// Cumulative supervisor-observed PTY output bytes.
    pub output_bytes: u64,
    complete_bytes: u64,
}

impl Verification {
    /// Lowercase hexadecimal chain head for receipts and out-of-band snapshots.
    #[must_use]
    pub fn head_hex(&self) -> String {
        hex(&self.head)
    }
}

/// Per-session summary returned after all frames are synchronized.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionAttestationSummary {
    /// Number of committed records.
    pub records: u64,
    /// Cumulative observed PTY input bytes.
    pub input_bytes: u64,
    /// Cumulative observed PTY output bytes.
    pub output_bytes: u64,
    /// Final hash-chain head.
    pub head: [u8; 32],
    /// Bytes occupied by complete frames.
    pub file_bytes: u64,
}

impl SessionAttestationSummary {
    /// Lowercase hexadecimal chain head for receipts and out-of-band snapshots.
    #[must_use]
    pub fn head_hex(&self) -> String {
        hex(&self.head)
    }
}

/// Complete synchronized summary, keyed by session identifier.
pub type AttestationSummary = BTreeMap<String, SessionAttestationSummary>;

/// Cloneable observation route bound to an exact observe capability projection.
#[derive(Clone)]
pub struct AttestationObserver {
    sender: Sender<Request>,
    observed: ObservedSessions,
}

impl AttestationObserver {
    /// Records a supervisor-owned lifecycle transition.
    pub fn lifecycle(&self, session_id: &str, phase: LifecyclePhase) -> io::Result<()> {
        self.send(session_id, Observation::Lifecycle(phase))
    }

    /// Records an authorized child PID immediately after spawn.
    pub fn spawn(&self, session_id: &str, pid: u32) -> io::Result<()> {
        self.send(session_id, Observation::Spawn(pid))
    }

    /// Records exact PTY input bytes observed at the supervisor write boundary.
    pub fn input(&self, session_id: &str, bytes: &[u8]) -> io::Result<()> {
        self.bytes(session_id, ByteDirection::Input, bytes)
    }

    /// Records exact PTY output bytes observed at the supervisor read boundary.
    pub fn output(&self, session_id: &str, bytes: &[u8]) -> io::Result<()> {
        self.bytes(session_id, ByteDirection::Output, bytes)
    }

    /// Records the child outcome obtained from the supervisor's child handle.
    pub fn exit(&self, session_id: &str, outcome: ExitOutcome) -> io::Result<()> {
        self.send(session_id, Observation::Exit(outcome))
    }

    fn bytes(&self, session_id: &str, direction: ByteDirection, bytes: &[u8]) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.send(
            session_id,
            Observation::Bytes {
                direction,
                bytes: bytes.to_vec().into_boxed_slice(),
            },
        )
    }

    fn send(&self, session_id: &str, observation: Observation) -> io::Result<()> {
        if !self.observed.contains(session_id) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "observe capability does not cover attestation session",
            ));
        }
        self.sender
            .send(Request::Observe {
                session_id: session_id.to_owned(),
                observation,
            })
            .map_err(|_| io::Error::other("attestation writer stopped"))
    }
}

/// Dedicated owner of append-only attestation files.
pub struct AttestationWriter {
    sender: Option<Sender<Request>>,
    worker: Option<JoinHandle<()>>,
    observed: ObservedSessions,
}

impl AttestationWriter {
    /// Creates one new protected log per observed session.
    ///
    /// Existing files fail closed. Files are held open for append by the writer and
    /// changed to mode `0400`, preventing a child from opening its path for writes.
    /// The caller must finish the writer to obtain synchronized chain heads.
    pub fn start<I, S>(
        directory: &Path,
        session_ids: I,
        capability: &ObserveCapability,
    ) -> io::Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        fs::create_dir_all(directory)?;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        let observed = observed_sessions(capability);
        let mut requested = BTreeSet::new();
        let mut logs = BTreeMap::new();
        for session_id in session_ids {
            let session_id = session_id.into();
            if !observed.contains(&session_id) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "observe capability does not cover attestation session",
                ));
            }
            if !requested.insert(session_id.clone()) {
                return Err(invalid_input("duplicate attestation session"));
            }
            let path = attestation_path(directory, &session_id);
            let file = OpenOptions::new()
                .create_new(true)
                .append(true)
                .mode(0o600)
                .open(&path)?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o400))?;
            logs.insert(session_id.clone(), SessionLog::new(file, session_id));
        }
        if requested.is_empty() || requested.len() != observed.len() {
            return Err(invalid_input(
                "attestation sessions must equal observe capability scope",
            ));
        }
        let clock = ObservationClock::start()?;
        let (sender, receiver) = mpsc::channel();
        let worker = thread::spawn(move || worker_loop(receiver, logs, clock));
        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
            observed,
        })
    }

    /// Mints a cloneable observe route carrying the exact proof projection.
    pub fn observer(&self) -> io::Result<AttestationObserver> {
        Ok(AttestationObserver {
            sender: self
                .sender
                .as_ref()
                .ok_or_else(|| io::Error::other("attestation writer is closed"))?
                .clone(),
            observed: self.observed.clone(),
        })
    }

    /// Synchronizes all records, closes the writer, and returns chain heads.
    pub fn finish(mut self) -> io::Result<AttestationSummary> {
        let result = self.request_finish();
        self.join_worker()?;
        result
    }

    fn request_finish(&mut self) -> io::Result<AttestationSummary> {
        let sender = self
            .sender
            .take()
            .ok_or_else(|| io::Error::other("attestation writer is closed"))?;
        let (reply, receiver) = mpsc::channel();
        sender
            .send(Request::Finish(reply))
            .map_err(|_| io::Error::other("attestation writer stopped before finish"))?;
        drop(sender);
        receiver
            .recv()
            .map_err(|_| io::Error::other("attestation writer stopped before finish"))?
    }

    fn join_worker(&mut self) -> io::Result<()> {
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| io::Error::other("attestation writer panicked"))?;
        }
        Ok(())
    }
}

impl Drop for AttestationWriter {
    fn drop(&mut self) {
        if self.sender.is_some() {
            let _ = self.request_finish();
        }
        let _ = self.join_worker();
    }
}

#[derive(Clone, Copy)]
enum ByteDirection {
    Input,
    Output,
}

impl ByteDirection {
    fn as_str(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
        }
    }
}

enum Observation {
    Lifecycle(LifecyclePhase),
    Spawn(u32),
    Bytes {
        direction: ByteDirection,
        bytes: Box<[u8]>,
    },
    Exit(ExitOutcome),
}

enum Request {
    Observe {
        session_id: String,
        observation: Observation,
    },
    Finish(Sender<io::Result<AttestationSummary>>),
}

struct ObservationClock {
    monotonic_origin: Instant,
    unix_origin_micros: u64,
}

impl ObservationClock {
    fn start() -> io::Result<Self> {
        let unix_origin_micros = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)
            .and_then(|duration| u64::try_from(duration.as_micros()).map_err(io::Error::other))?;
        Ok(Self {
            monotonic_origin: Instant::now(),
            unix_origin_micros,
        })
    }

    fn observed_at(&self) -> io::Result<(u64, u64)> {
        let elapsed = self.monotonic_origin.elapsed();
        let monotonic_nanos = u64::try_from(elapsed.as_nanos()).map_err(io::Error::other)?;
        let elapsed_micros = u64::try_from(elapsed.as_micros()).map_err(io::Error::other)?;
        let unix_micros = self
            .unix_origin_micros
            .checked_add(elapsed_micros)
            .ok_or_else(|| io::Error::other("attestation timestamp overflow"))?;
        Ok((monotonic_nanos, unix_micros))
    }
}

struct SessionLog {
    file: File,
    session_id: String,
    sequence: u64,
    head: [u8; 32],
    input_hasher: Sha256,
    output_hasher: Sha256,
    input_bytes: u64,
    output_bytes: u64,
    file_bytes: u64,
}

impl SessionLog {
    fn new(file: File, session_id: String) -> Self {
        Self {
            file,
            session_id,
            sequence: 0,
            head: ZERO_HASH,
            input_hasher: Sha256::new(),
            output_hasher: Sha256::new(),
            input_bytes: 0,
            output_bytes: 0,
            file_bytes: 0,
        }
    }

    fn append(
        &mut self,
        observation: Observation,
        monotonic_nanos: u64,
        unix_micros: u64,
    ) -> io::Result<()> {
        let sequence = self.sequence;
        let payload = match observation {
            Observation::Lifecycle(phase) => canonical_payload(
                sequence,
                monotonic_nanos,
                unix_micros,
                PayloadFields::non_byte("lifecycle", phase.as_str()),
            ),
            Observation::Spawn(pid) => canonical_payload(
                sequence,
                monotonic_nanos,
                unix_micros,
                PayloadFields::non_byte("spawn", &pid.to_string()),
            ),
            Observation::Bytes { direction, bytes } => {
                let delta = u64::try_from(bytes.len()).map_err(io::Error::other)?;
                let (hasher, total) = match direction {
                    ByteDirection::Input => (&mut self.input_hasher, &mut self.input_bytes),
                    ByteDirection::Output => (&mut self.output_hasher, &mut self.output_bytes),
                };
                hasher.update(&bytes)?;
                *total = total
                    .checked_add(delta)
                    .ok_or_else(|| io::Error::other("attestation byte count overflow"))?;
                let rolling_hash = hex(&hasher.digest()?);
                canonical_payload(
                    sequence,
                    monotonic_nanos,
                    unix_micros,
                    PayloadFields {
                        kind: direction.as_str(),
                        value: "-",
                        delta,
                        total: *total,
                        content_hash: &rolling_hash,
                    },
                )
            }
            Observation::Exit(outcome) => canonical_payload(
                sequence,
                monotonic_nanos,
                unix_micros,
                PayloadFields::non_byte("exit", &outcome.encode()),
            ),
        };
        let payload = format!("{}\t{payload}", self.session_id);
        if payload.len() > MAX_PAYLOAD_BYTES {
            return Err(invalid_data("attestation payload exceeds limit"));
        }
        let record_hash = digest_parts(&[CHAIN_DOMAIN, &self.head, payload.as_bytes()])?;
        self.file.write_all(FRAME_MAGIC)?;
        let payload_len = u32::try_from(payload.len()).map_err(io::Error::other)?;
        self.file.write_all(&payload_len.to_le_bytes())?;
        self.file.write_all(&self.head)?;
        self.file.write_all(payload.as_bytes())?;
        self.file.write_all(&record_hash)?;
        self.file.write_all(COMMIT_MAGIC)?;
        self.file.sync_data()?;
        let frame_bytes = 4_u64
            .checked_add(4)
            .and_then(|value| value.checked_add(32))
            .and_then(|value| value.checked_add(u64::from(payload_len)))
            .and_then(|value| value.checked_add(32))
            .and_then(|value| value.checked_add(4))
            .ok_or_else(|| io::Error::other("attestation frame size overflow"))?;
        self.file_bytes = self
            .file_bytes
            .checked_add(frame_bytes)
            .ok_or_else(|| io::Error::other("attestation file size overflow"))?;
        self.head = record_hash;
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| io::Error::other("attestation sequence overflow"))?;
        Ok(())
    }

    fn summary(&self) -> SessionAttestationSummary {
        SessionAttestationSummary {
            records: self.sequence,
            input_bytes: self.input_bytes,
            output_bytes: self.output_bytes,
            head: self.head,
            file_bytes: self.file_bytes,
        }
    }
}

fn worker_loop(
    receiver: Receiver<Request>,
    mut logs: BTreeMap<String, SessionLog>,
    clock: ObservationClock,
) {
    let mut failure: Option<(io::ErrorKind, String)> = None;
    while let Ok(request) = receiver.recv() {
        match request {
            Request::Observe {
                session_id,
                observation,
            } => {
                if failure.is_none() {
                    let result = (|| {
                        let (monotonic_nanos, unix_micros) = clock.observed_at()?;
                        logs.get_mut(&session_id)
                            .ok_or_else(|| invalid_data("attestation names unknown session"))?
                            .append(observation, monotonic_nanos, unix_micros)
                    })();
                    if let Err(error) = result {
                        failure = Some((error.kind(), error.to_string()));
                    }
                }
            }
            Request::Finish(reply) => {
                let result = match failure {
                    Some((kind, message)) => Err(io::Error::new(kind, message)),
                    None => Ok(logs
                        .iter()
                        .map(|(session_id, log)| (session_id.clone(), log.summary()))
                        .collect()),
                };
                let _ = reply.send(result);
                return;
            }
        }
    }
}

struct PayloadFields<'a> {
    kind: &'a str,
    value: &'a str,
    delta: u64,
    total: u64,
    content_hash: &'a str,
}

impl<'a> PayloadFields<'a> {
    fn non_byte(kind: &'a str, value: &'a str) -> Self {
        Self {
            kind,
            value,
            delta: 0,
            total: 0,
            content_hash: "-",
        }
    }
}

fn canonical_payload(
    sequence: u64,
    monotonic_nanos: u64,
    unix_micros: u64,
    fields: PayloadFields<'_>,
) -> String {
    let PayloadFields {
        kind,
        value,
        delta,
        total,
        content_hash,
    } = fields;
    format!(
        "{sequence}\t{monotonic_nanos}\t{unix_micros}\t{kind}\t{value}\t{delta}\t{total}\t{content_hash}"
    )
}

/// Returns the stable file path for one validated session identifier.
#[must_use]
pub fn attestation_path(directory: &Path, session_id: &str) -> PathBuf {
    directory.join(format!("{session_id}.attest"))
}

/// Verifies framing, canonical payloads, sequence, byte totals, and every chain link.
///
/// Mutation, insertion, reordering, malformed payloads, and invalid committed
/// frames return `InvalidData`. An incomplete final frame returns a valid-prefix
/// result with [`LogIntegrity::TornTail`]. Truncation exactly at a frame boundary
/// requires an out-of-band retained head to detect, as stated in the wave axioms.
pub fn verify_attestation(path: &Path) -> io::Result<Verification> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > MAX_LOG_BYTES {
        return Err(invalid_data("attestation log exceeds size limit"));
    }
    let bytes = fs::read(path)?;
    let session_id = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_suffix(".attest"))
        .ok_or_else(|| invalid_data("attestation path has no session identity"))?;
    verify_bytes(&bytes, session_id)
}

/// Removes only an incomplete final frame, preserving the verified complete prefix.
///
/// Complete files and any committed/hash-invalid mutation are rejected. The file is
/// returned to read-only mode after recovery.
pub fn repair_torn_tail(path: &Path) -> io::Result<Verification> {
    let verification = verify_attestation(path)?;
    if verification.integrity != LogIntegrity::TornTail {
        return Err(invalid_data("attestation log has no torn tail"));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    let repair_result = OpenOptions::new().write(true).open(path).and_then(|file| {
        file.set_len(verification.complete_bytes)?;
        file.sync_all()
    });
    let protect_result = fs::set_permissions(path, fs::Permissions::from_mode(0o400));
    repair_result?;
    protect_result?;
    verify_attestation(path)
}

fn verify_bytes(bytes: &[u8], expected_session_id: &str) -> io::Result<Verification> {
    let mut offset = 0_usize;
    let mut expected_sequence = 0_u64;
    let mut expected_head = ZERO_HASH;
    let mut input_bytes = 0_u64;
    let mut output_bytes = 0_u64;
    let mut previous_monotonic = None;
    while offset < bytes.len() {
        let remaining = bytes.len() - offset;
        if remaining < minimum_frame_bytes() {
            return torn_verification(
                expected_sequence,
                expected_head,
                input_bytes,
                output_bytes,
                offset,
            );
        }
        if &bytes[offset..offset + 4] != FRAME_MAGIC {
            return Err(invalid_data("invalid attestation frame magic"));
        }
        let payload_len = usize::try_from(u32::from_le_bytes(
            bytes[offset + 4..offset + 8]
                .try_into()
                .map_err(|_| invalid_data("invalid attestation length bytes"))?,
        ))
        .map_err(|_| invalid_data("attestation payload length overflow"))?;
        if payload_len == 0 || payload_len > MAX_PAYLOAD_BYTES {
            return Err(invalid_data("invalid attestation payload length"));
        }
        let frame_len = 4_usize
            .checked_add(4)
            .and_then(|value| value.checked_add(32))
            .and_then(|value| value.checked_add(payload_len))
            .and_then(|value| value.checked_add(32))
            .and_then(|value| value.checked_add(4))
            .ok_or_else(|| invalid_data("attestation frame length overflow"))?;
        if remaining < frame_len {
            return torn_verification(
                expected_sequence,
                expected_head,
                input_bytes,
                output_bytes,
                offset,
            );
        }
        let previous_start = offset + 8;
        let payload_start = previous_start + 32;
        let hash_start = payload_start + payload_len;
        let commit_start = hash_start + 32;
        let previous: [u8; 32] = bytes[previous_start..payload_start]
            .try_into()
            .map_err(|_| invalid_data("invalid attestation previous hash"))?;
        if previous != expected_head {
            return Err(invalid_data("attestation previous hash differs"));
        }
        if &bytes[commit_start..commit_start + 4] != COMMIT_MAGIC {
            return Err(invalid_data("invalid attestation commit marker"));
        }
        let payload = &bytes[payload_start..hash_start];
        let actual_hash: [u8; 32] = bytes[hash_start..commit_start]
            .try_into()
            .map_err(|_| invalid_data("invalid attestation record hash"))?;
        let expected_hash = digest_parts(&[CHAIN_DOMAIN, &previous, payload])?;
        if actual_hash != expected_hash {
            return Err(invalid_data("attestation record hash differs"));
        }
        validate_payload(
            payload,
            expected_session_id,
            expected_sequence,
            &mut input_bytes,
            &mut output_bytes,
            &mut previous_monotonic,
        )?;
        expected_head = actual_hash;
        expected_sequence = expected_sequence
            .checked_add(1)
            .ok_or_else(|| invalid_data("attestation record count overflow"))?;
        offset = offset
            .checked_add(frame_len)
            .ok_or_else(|| invalid_data("attestation offset overflow"))?;
    }
    Ok(Verification {
        integrity: LogIntegrity::Complete,
        records: expected_sequence,
        head: expected_head,
        input_bytes,
        output_bytes,
        complete_bytes: u64::try_from(offset).map_err(io::Error::other)?,
    })
}

fn validate_payload(
    payload: &[u8],
    expected_session_id: &str,
    expected_sequence: u64,
    input_bytes: &mut u64,
    output_bytes: &mut u64,
    previous_monotonic: &mut Option<u64>,
) -> io::Result<()> {
    let payload = std::str::from_utf8(payload)
        .map_err(|_| invalid_data("attestation payload is not UTF-8"))?;
    let fields = payload.split('\t').collect::<Vec<_>>();
    let [session_id, sequence, monotonic, unix, kind, value, delta, total, content_hash] =
        fields.as_slice()
    else {
        return Err(invalid_data("invalid attestation payload shape"));
    };
    if *session_id != expected_session_id {
        return Err(invalid_data("attestation session identity differs"));
    }
    if parse_u64(sequence, "sequence")? != expected_sequence {
        return Err(invalid_data("attestation sequence differs"));
    }
    let monotonic = parse_u64(monotonic, "monotonic timestamp")?;
    if previous_monotonic.is_some_and(|previous| monotonic < previous) {
        return Err(invalid_data("attestation monotonic timestamp regressed"));
    }
    *previous_monotonic = Some(monotonic);
    let _unix_micros = parse_u64(unix, "Unix timestamp")?;
    let delta = parse_u64(delta, "byte delta")?;
    let total = parse_u64(total, "byte total")?;
    match *kind {
        "input" | "output" => {
            if *value != "-" || delta == 0 || !is_lower_hex_digest(content_hash) {
                return Err(invalid_data("invalid attestation byte observation"));
            }
            let running = if *kind == "input" {
                input_bytes
            } else {
                output_bytes
            };
            *running = running
                .checked_add(delta)
                .ok_or_else(|| invalid_data("attestation byte total overflow"))?;
            if *running != total {
                return Err(invalid_data("attestation byte total differs"));
            }
        }
        "lifecycle" => {
            if !matches!(*value, "created" | "running" | "ended")
                || delta != 0
                || total != 0
                || *content_hash != "-"
            {
                return Err(invalid_data("invalid lifecycle attestation"));
            }
        }
        "spawn" => {
            if parse_u64(value, "spawn PID")? == 0
                || delta != 0
                || total != 0
                || *content_hash != "-"
            {
                return Err(invalid_data("invalid spawn attestation"));
            }
        }
        "exit" => {
            let valid_outcome = value
                .strip_prefix("code:")
                .or_else(|| value.strip_prefix("signal:"))
                .is_some_and(|number| number.parse::<i32>().is_ok());
            if !valid_outcome || delta != 0 || total != 0 || *content_hash != "-" {
                return Err(invalid_data("invalid exit attestation"));
            }
        }
        _ => return Err(invalid_data("unknown attestation event kind")),
    }
    Ok(())
}

fn torn_verification(
    records: u64,
    head: [u8; 32],
    input_bytes: u64,
    output_bytes: u64,
    complete_bytes: usize,
) -> io::Result<Verification> {
    Ok(Verification {
        integrity: LogIntegrity::TornTail,
        records,
        head,
        input_bytes,
        output_bytes,
        complete_bytes: u64::try_from(complete_bytes).map_err(io::Error::other)?,
    })
}

fn minimum_frame_bytes() -> usize {
    4 + 4 + 32 + 1 + 32 + 4
}

fn parse_u64(value: &str, name: &str) -> io::Result<u64> {
    if value.len() > 20 || (value.len() > 1 && value.starts_with('0')) {
        return Err(invalid_data(format!("invalid attestation {name}")));
    }
    value
        .parse::<u64>()
        .map_err(|_| invalid_data(format!("invalid attestation {name}")))
}

fn is_lower_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
