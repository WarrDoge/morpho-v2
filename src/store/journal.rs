//! Append-only JSON-lines journal: the only durable record of everything except vectors.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::pyfmt::Row;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Commit {
    pub proposal: Row,
    pub transitions: Vec<Row>,
    #[serde(default)]
    pub vectors: BTreeMap<String, usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Record {
    Event(Row),
    Commit(Commit),
    Cursor(Row),
    Snapshot(Row),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub seq: u64,
    #[serde(flatten)]
    pub record: Record,
}

pub struct Journal {
    file: File,
    path: PathBuf,
    pub len: u64,
    pub seq: u64,
}

impl Journal {
    /// Opens (or creates) the journal, returning every complete entry with its byte offset.
    /// A torn final line is truncated away.
    pub fn open(path: &Path) -> Result<(Journal, Vec<(u64, Entry)>)> {
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(path)?;
        let mut buf = String::new();
        file.read_to_string(&mut buf)?;
        let mut entries = Vec::new();
        let mut offset = 0u64;
        for line in buf.split_inclusive('\n') {
            if !line.ends_with('\n') {
                break;
            }
            match serde_json::from_str::<Entry>(line) {
                Ok(e) => entries.push((offset, e)),
                Err(err) => bail!("corrupt journal {} at byte {offset}: {err}", path.display()),
            }
            offset += line.len() as u64;
        }
        if offset < buf.len() as u64 {
            file.set_len(offset)?;
        }
        let seq = entries.last().map_or(0, |(_, e)| e.seq);
        Ok((
            Journal {
                file,
                path: path.to_path_buf(),
                len: offset,
                seq,
            },
            entries,
        ))
    }

    /// Appends and syncs one record; returns its sequence number and byte offset.
    pub fn append(&mut self, record: &Record) -> Result<(u64, u64)> {
        self.seq += 1;
        let mut line = serde_json::to_string(&Entry {
            seq: self.seq,
            record: record.clone(),
        })?;
        line.push('\n');
        self.file.write_all(line.as_bytes())?;
        self.file.sync_data()?;
        let offset = self.len;
        self.len += line.len() as u64;
        Ok((self.seq, offset))
    }

    pub fn read_at(&self, offset: u64) -> Result<Entry> {
        let mut f = File::open(&self.path)?;
        f.seek(SeekFrom::Start(offset))?;
        let mut line = String::new();
        BufReader::new(f).read_line(&mut line)?;
        Ok(serde_json::from_str(&line)?)
    }
}
