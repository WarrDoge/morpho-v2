//! Fixed-stride little-endian f32 vector file, memory-mapped for scans.

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

use anyhow::{Result, ensure};
use memmap2::Mmap;

pub struct Vectors {
    file: File,
    map: Option<Mmap>,
    dim: usize,
    pub slots: usize,
}

impl Vectors {
    pub fn open(path: &Path, dim: usize) -> Result<Vectors> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)?;
        let stride = (dim * 4) as u64;
        let len = file.metadata()?.len();
        let whole = len - len % stride;
        if whole != len {
            file.set_len(whole)?;
        }
        let mut v = Vectors {
            file,
            map: None,
            dim,
            slots: (whole / stride) as usize,
        };
        v.remap()?;
        Ok(v)
    }

    fn remap(&mut self) -> Result<()> {
        // SAFETY: the file is only ever appended to by this process; mapped bytes never change.
        self.map = if self.slots == 0 {
            None
        } else {
            Some(unsafe { Mmap::map(&self.file)? })
        };
        Ok(())
    }

    pub fn append(&mut self, v: &[f32]) -> Result<usize> {
        ensure!(
            v.len() == self.dim,
            "vector has {} dims, expected {}",
            v.len(),
            self.dim
        );
        let mut bytes = Vec::with_capacity(self.dim * 4);
        for x in v {
            bytes.extend_from_slice(&x.to_le_bytes());
        }
        self.file.seek(SeekFrom::End(0))?;
        self.file.write_all(&bytes)?;
        self.file.sync_data()?;
        let slot = self.slots;
        self.slots += 1;
        self.remap()?;
        Ok(slot)
    }

    fn bytes(&self, slot: usize) -> &[u8] {
        let stride = self.dim * 4;
        &self.map.as_ref().expect("slot out of range")[slot * stride..(slot + 1) * stride]
    }

    pub fn get(&self, slot: usize) -> Vec<f32> {
        self.bytes(slot)
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect()
    }

    /// pgvector's cosine similarity: dot / sqrt(|a|²|b|²), clamped to [-1, 1].
    pub fn similarity(&self, slot: usize, q: &[f32]) -> f64 {
        let (mut dot, mut na, mut nb) = (0f64, 0f64, 0f64);
        for (c, b) in self.bytes(slot).as_chunks::<4>().0.iter().zip(q) {
            let a = f32::from_le_bytes(*c) as f64;
            let b = *b as f64;
            dot += a * b;
            na += a * a;
            nb += b * b;
        }
        (dot / (na * nb).sqrt()).clamp(-1.0, 1.0)
    }

    pub fn similarity_between(&self, a: usize, b: usize) -> f64 {
        self.similarity(a, &self.get(b))
    }
}

/// Python's `1 - (a <=> b)`, kept as the same two subtractions.
pub fn relevance(similarity: f64) -> f64 {
    1.0 - (1.0 - similarity)
}
