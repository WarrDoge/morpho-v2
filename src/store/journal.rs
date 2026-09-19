//! Ordered libSQL state records and a read-only importer for legacy JSON-lines journals.

use futures_executor::block_on;
use libsql::Connection;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

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

#[derive(Clone)]
pub struct Journal {
    pub db: Arc<Connection>,
    pub seq: u64,
    pub pending: Option<Vec<Record>>,
}

impl Journal {
    pub fn entries(db: &Connection) -> Result<Vec<(u64, Entry)>> {
        block_on(async {
            let mut rows = db
                .query("SELECT seq, record FROM records ORDER BY seq", ())
                .await?;
            let mut entries = Vec::new();
            while let Some(row) = rows.next().await? {
                let seq = row.get::<i64>(0)? as u64;
                entries.push((
                    seq,
                    Entry {
                        seq,
                        record: serde_json::from_str(&row.get::<String>(1)?)?,
                    },
                ));
            }
            Ok(entries)
        })
    }

    pub fn append(&mut self, record: &Record) -> Result<(u64, u64)> {
        let seq = self.seq + 1;
        if let Some(pending) = &mut self.pending {
            pending.push(record.clone());
        } else {
            block_on(self.db.execute(
                "INSERT INTO records(seq,record) VALUES (?,?)",
                libsql::params![seq as i64, serde_json::to_string(record)?],
            ))?;
        }
        self.seq = seq;
        Ok((seq, seq))
    }

    pub fn read_at(&self, offset: u64) -> Result<Entry> {
        if let Some(pending) = &self.pending {
            let base = self.seq - pending.len() as u64;
            if offset > base && offset <= self.seq {
                return Ok(Entry {
                    seq: offset,
                    record: pending[(offset - base - 1) as usize].clone(),
                });
            }
        }
        block_on(async {
            let mut rows = self
                .db
                .query("SELECT record FROM records WHERE seq=?", [offset as i64])
                .await?;
            let row = rows
                .next()
                .await?
                .ok_or_else(|| anyhow::anyhow!("missing record {offset}"))?;
            Ok(Entry {
                seq: offset,
                record: serde_json::from_str(&row.get::<String>(0)?)?,
            })
        })
    }
}

/// Read complete legacy records without changing the source, including a torn UTF-8 tail.
pub fn read_legacy(path: &Path) -> Result<Vec<(u64, Entry)>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes = std::fs::read(path)?;
    let mut entries = Vec::new();
    let mut offset = 0;
    for line in bytes.split_inclusive(|b| *b == b'\n') {
        if !line.ends_with(b"\n") {
            tracing::warn!("ignoring incomplete legacy journal tail at byte {offset}");
            break;
        }
        let e: Entry = serde_json::from_slice(line)
            .map_err(|e| anyhow::anyhow!("corrupt journal at byte {offset}: {e}"))?;
        if e.seq != entries.len() as u64 + 1 {
            bail!("non-sequential legacy record at {offset}");
        }
        entries.push((e.seq, e));
        offset += line.len();
    }
    Ok(entries)
}
