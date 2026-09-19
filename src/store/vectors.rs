//! Native libSQL vectors. Pending vectors belong to an unpublished coordinator turn.
use anyhow::{Result, ensure};
use futures_executor::block_on;
use libsql::Connection;
use std::{collections::HashMap, sync::Arc};

#[derive(Clone)]
pub struct Vectors {
    pub db: Arc<Connection>,
    pub dim: usize,
    pub slots: usize,
    pub pending: Option<Vec<Vec<f32>>>,
}

pub fn bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|v| v.to_le_bytes()).collect()
}

impl Vectors {
    pub fn append(&mut self, v: &[f32]) -> Result<usize> {
        ensure!(
            v.len() == self.dim && v.iter().all(|x| x.is_finite()),
            "invalid embedding dimensions or values"
        );
        let slot = self.slots;
        if let Some(pending) = &mut self.pending {
            pending.push(v.to_vec());
        } else {
            block_on(self.db.execute(
                "INSERT INTO vectors(slot,embedding) VALUES (?,vector32(?))",
                libsql::params![slot as i64, bytes(v)],
            ))?;
        }
        self.slots += 1;
        Ok(slot)
    }

    pub fn get(&self, slot: usize) -> Result<Vec<f32>> {
        if let Some(pending) = &self.pending {
            let base = self.slots - pending.len();
            if slot >= base {
                return pending
                    .get(slot - base)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("missing vector {slot}"));
            }
        }
        block_on(async {
            let mut rows = self
                .db
                .query("SELECT embedding FROM vectors WHERE slot=?", [slot as i64])
                .await?;
            let row = rows
                .next()
                .await?
                .ok_or_else(|| anyhow::anyhow!("missing vector {slot}"))?;
            let bytes = row.get::<Vec<u8>>(0)?;
            ensure!(bytes.len() == self.dim * 4, "invalid stored vector {slot}");
            Ok(bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c))
                .collect())
        })
    }

    pub fn distances(&self, q: &[f32]) -> Result<HashMap<usize, f64>> {
        ensure!(
            q.len() == self.dim && q.iter().all(|x| x.is_finite()),
            "invalid query embedding"
        );
        block_on(async {
            let mut rows = self
                .db
                .query(
                    "SELECT slot, vector_distance_cos(embedding,vector32(?)) FROM vectors",
                    [bytes(q)],
                )
                .await?;
            let mut distances = HashMap::new();
            while let Some(row) = rows.next().await? {
                let distance = row.get::<f64>(1).unwrap_or(1.0);
                distances.insert(
                    row.get::<i64>(0)? as usize,
                    if distance.is_finite() { distance } else { 1.0 },
                );
            }
            if let Some(pending) = &self.pending {
                let base = self.slots - pending.len();
                for (i, v) in pending.iter().enumerate() {
                    let mut rows = self
                        .db
                        .query(
                            "SELECT vector_distance_cos(vector32(?),vector32(?))",
                            libsql::params![bytes(v), bytes(q)],
                        )
                        .await?;
                    let d = rows.next().await?.unwrap().get::<f64>(0).unwrap_or(1.0);
                    distances.insert(base + i, if d.is_finite() { d } else { 1.0 });
                }
            }
            ensure!(
                (0..self.slots).all(|slot| distances.contains_key(&slot)),
                "missing stored vectors"
            );
            Ok(distances)
        })
    }

    pub fn similarity_between(&self, a: usize, b: usize) -> Result<f64> {
        let distances = self.distances(&self.get(b)?)?;
        let distance = distances
            .get(&a)
            .ok_or_else(|| anyhow::anyhow!("missing vector {a}"))?;
        Ok(1.0 - distance)
    }
}

pub fn relevance(similarity: f64) -> f64 {
    1.0 - (1.0 - similarity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_distances_and_slot_accounting() -> Result<()> {
        let db = block_on(libsql::Builder::new_local(":memory:").build())?;
        let db = Arc::new(db.connect()?);
        block_on(db.execute(
            "CREATE TABLE vectors(slot INTEGER PRIMARY KEY, embedding F32_BLOB NOT NULL)",
            (),
        ))?;
        let mut vectors = Vectors {
            db,
            dim: 2,
            slots: 0,
            pending: None,
        };
        assert!(vectors.distances(&[1.0, 0.0])?.is_empty());
        assert!(vectors.distances(&[1.0]).is_err());
        assert!(vectors.append(&[f32::NAN, 0.0]).is_err());
        assert_eq!(vectors.slots, 0);
        assert_eq!(vectors.append(&[1.0, 0.0])?, 0);
        assert_eq!(vectors.get(0)?, [1.0, 0.0]);
        assert_eq!(relevance(vectors.similarity_between(0, 0)?), 1.0);
        assert!(vectors.similarity_between(1, 0).is_err());
        assert!(vectors.get(1).is_err());

        block_on(vectors.db.execute(
            "CREATE TRIGGER fail_append BEFORE INSERT ON vectors BEGIN SELECT RAISE(ABORT,'test'); END",
            (),
        ))?;
        assert!(vectors.append(&[0.0, 1.0]).is_err());
        assert_eq!(vectors.slots, 1);
        vectors.pending = Some(Vec::new());
        assert_eq!(vectors.append(&[-1.0, 0.0])?, 1);
        assert_eq!(vectors.get(1)?, [-1.0, 0.0]);
        assert_eq!(vectors.distances(&[1.0, 0.0])?.len(), 2);
        assert_eq!(relevance(vectors.similarity_between(0, 1)?), -1.0);

        block_on(
            vectors
                .db
                .execute("UPDATE vectors SET embedding=x'00' WHERE slot=0", ()),
        )?;
        assert!(vectors.get(0).is_err());
        block_on(vectors.db.execute("DELETE FROM vectors WHERE slot=0", ()))?;
        assert!(vectors.distances(&[1.0, 0.0]).is_err());
        Ok(())
    }
}
