//! Asynchronous append-only scrollback segment writer.
//!
//! Contract: ingest enqueues bounded records and performs no filesystem I/O. The
//! worker appends framed segments, synchronizes them on an explicit barrier, and
//! reports exclusive persisted offsets for the passive state pointer.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use crate::state::PendingScrollback;

const SEGMENT_MAGIC: &[u8; 4] = b"RMS2";
const MAX_SEGMENT_RECORDS: u32 = 256;
const MAX_RECORD_BYTES: usize = 4_096;

pub struct ScrollbackWriter {
    sender: Option<Sender<Request>>,
    worker: Option<JoinHandle<()>>,
}

impl ScrollbackWriter {
    pub fn start<I, S>(directory: &Path, session_ids: I) -> io::Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        fs::create_dir_all(directory)?;
        let mut files = BTreeMap::new();
        let mut offsets = BTreeMap::new();
        for session_id in session_ids {
            let session_id = session_id.into();
            let path = directory.join(segment_file_name(&session_id));
            let file = OpenOptions::new()
                .create_new(true)
                .append(true)
                .open(path)?;
            if files.insert(session_id.clone(), file).is_some() {
                return Err(invalid_data("duplicate scrollback session"));
            }
            offsets.insert(session_id, 0);
        }
        let (sender, receiver) = mpsc::channel();
        let worker = thread::spawn(move || worker_loop(receiver, files, offsets));
        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
        })
    }

    pub fn enqueue(&self, records: Vec<PendingScrollback>) -> io::Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        self.send(Request::Append(records))
    }

    pub fn flush(&self) -> io::Result<BTreeMap<String, u64>> {
        let (sender, receiver) = mpsc::channel();
        self.send(Request::Flush(sender))?;
        receiver
            .recv()
            .map_err(|_| io::Error::other("scrollback worker stopped before flush"))?
    }

    pub fn finish(mut self) -> io::Result<BTreeMap<String, u64>> {
        let (sender, receiver) = mpsc::channel();
        self.send(Request::Finish(sender))?;
        self.sender.take();
        let result = receiver
            .recv()
            .map_err(|_| io::Error::other("scrollback worker stopped before finish"))?;
        self.join_worker()?;
        result
    }

    fn send(&self, request: Request) -> io::Result<()> {
        self.sender
            .as_ref()
            .ok_or_else(|| io::Error::other("scrollback writer is closed"))?
            .send(request)
            .map_err(|_| io::Error::other("scrollback worker stopped"))
    }

    fn join_worker(&mut self) -> io::Result<()> {
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| io::Error::other("scrollback worker panicked"))?;
        }
        Ok(())
    }
}

impl Drop for ScrollbackWriter {
    fn drop(&mut self) {
        self.sender.take();
        let _ = self.join_worker();
    }
}

enum Request {
    Append(Vec<PendingScrollback>),
    Flush(Sender<io::Result<BTreeMap<String, u64>>>),
    Finish(Sender<io::Result<BTreeMap<String, u64>>>),
}

fn worker_loop(
    receiver: Receiver<Request>,
    mut files: BTreeMap<String, File>,
    mut offsets: BTreeMap<String, u64>,
) {
    let mut failure: Option<(io::ErrorKind, String)> = None;
    while let Ok(request) = receiver.recv() {
        match request {
            Request::Append(records) => {
                if failure.is_none() {
                    if let Err(error) = append_records(&mut files, &mut offsets, records) {
                        failure = Some((error.kind(), error.to_string()));
                    }
                }
            }
            Request::Flush(reply) => {
                let result = flush_files(&mut files, &offsets, failure.as_ref());
                let _ = reply.send(result);
            }
            Request::Finish(reply) => {
                let result = flush_files(&mut files, &offsets, failure.as_ref());
                let _ = reply.send(result);
                return;
            }
        }
    }
}

fn append_records(
    files: &mut BTreeMap<String, File>,
    offsets: &mut BTreeMap<String, u64>,
    records: Vec<PendingScrollback>,
) -> io::Result<()> {
    let mut by_session = BTreeMap::<String, Vec<PendingScrollback>>::new();
    for record in records {
        by_session
            .entry(record.session_id.clone())
            .or_default()
            .push(record);
    }
    for (session_id, records) in by_session {
        let offset = offsets
            .get_mut(&session_id)
            .ok_or_else(|| invalid_data("scrollback record names unknown session"))?;
        let first = records
            .first()
            .ok_or_else(|| invalid_data("empty scrollback segment"))?;
        if first.offset != *offset {
            return Err(invalid_data("scrollback segment offset is not contiguous"));
        }
        let count = u32::try_from(records.len())
            .map_err(|_| invalid_data("scrollback segment has too many records"))?;
        if count > MAX_SEGMENT_RECORDS {
            return Err(invalid_data("scrollback segment has too many records"));
        }
        let file = files
            .get_mut(&session_id)
            .ok_or_else(|| invalid_data("scrollback file missing for session"))?;
        file.write_all(SEGMENT_MAGIC)?;
        file.write_all(&first.offset.to_le_bytes())?;
        file.write_all(&count.to_le_bytes())?;
        for record in records {
            if record.offset != *offset {
                return Err(invalid_data("scrollback record offset is not contiguous"));
            }
            let payload = record.payload.as_bytes();
            if payload.is_empty() || payload.len() > MAX_RECORD_BYTES {
                return Err(invalid_data("scrollback payload exceeds segment limit"));
            }
            let length = u32::try_from(payload.len())
                .map_err(|_| invalid_data("scrollback payload exceeds segment limit"))?;
            file.write_all(&length.to_le_bytes())?;
            file.write_all(payload)?;
            *offset = offset
                .checked_add(1)
                .ok_or_else(|| invalid_data("scrollback persisted offset overflow"))?;
        }
    }
    Ok(())
}

fn flush_files(
    files: &mut BTreeMap<String, File>,
    offsets: &BTreeMap<String, u64>,
    failure: Option<&(io::ErrorKind, String)>,
) -> io::Result<BTreeMap<String, u64>> {
    if let Some((kind, message)) = failure {
        return Err(io::Error::new(*kind, message.clone()));
    }
    for file in files.values_mut() {
        file.flush()?;
        file.sync_data()?;
    }
    Ok(offsets.clone())
}

pub fn read_segments(path: &Path, persisted_through: u64) -> io::Result<Vec<String>> {
    let mut file = File::open(path)?;
    let mut output = Vec::new();
    let mut expected_offset = 0_u64;
    while expected_offset < persisted_through {
        let mut magic = [0_u8; 4];
        file.read_exact(&mut magic)?;
        if &magic != SEGMENT_MAGIC {
            return Err(invalid_data("invalid scrollback segment magic"));
        }
        let start = read_u64(&mut file)?;
        let count = read_u32(&mut file)?;
        let segment_end = start
            .checked_add(u64::from(count))
            .ok_or_else(|| invalid_data("scrollback segment offset overflow"))?;
        if start != expected_offset
            || count == 0
            || count > MAX_SEGMENT_RECORDS
            || segment_end > persisted_through
        {
            return Err(invalid_data("invalid scrollback segment header"));
        }
        for _ in 0..count {
            let length = usize::try_from(read_u32(&mut file)?)
                .map_err(|_| invalid_data("scrollback record length overflow"))?;
            if length == 0 || length > MAX_RECORD_BYTES {
                return Err(invalid_data("invalid scrollback record length"));
            }
            let mut payload = vec![0_u8; length];
            file.read_exact(&mut payload)?;
            output.push(
                String::from_utf8(payload)
                    .map_err(|_| invalid_data("scrollback segment payload is not UTF-8"))?,
            );
            expected_offset = expected_offset
                .checked_add(1)
                .ok_or_else(|| invalid_data("scrollback read offset overflow"))?;
        }
    }
    Ok(output)
}

pub fn segment_file_name(session_id: &str) -> String {
    format!("{session_id}.segments")
}

pub fn segment_path(directory: &Path, session_id: &str) -> PathBuf {
    directory.join(segment_file_name(session_id))
}

fn read_u64(reader: &mut impl Read) -> io::Result<u64> {
    let mut bytes = [0_u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn read_u32(reader: &mut impl Read) -> io::Result<u32> {
    let mut bytes = [0_u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
