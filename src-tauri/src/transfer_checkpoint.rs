use crate::transfers::{Handle, CHUNK_BYTES};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
};

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Checkpoint {
    pub(crate) source: Option<Value>,
    pub(crate) temp: String,
    pub(crate) hashes: Vec<String>,
    pub(crate) digest: Option<String>,
    pub(crate) commit_pending: bool,
    pub(crate) verified_bytes: u64,
    pub(crate) temp_size: Option<u64>,
}

pub(crate) fn block_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn local_identity(file: &File) -> Result<Value> {
    let metadata = file.metadata()?;
    Ok(
        json!({"size":metadata.len(),"modified":metadata.modified()?.duration_since(std::time::UNIX_EPOCH)?.as_nanos().to_string()}),
    )
}

pub(crate) fn source_identity(h: &Handle, identity: Value) -> Result<()> {
    {
        let mut checkpoint = h.checkpoint.lock().unwrap();
        if let Some(before) = &checkpoint.source {
            anyhow::ensure!(
                *before == identity,
                "SOURCE_CHANGED: Source file changed; create a new transfer"
            );
        } else {
            checkpoint.source = Some(identity);
        }
    }
    h.persist()
}

fn verified_size(hashes: &[String], total: u64) -> u64 {
    let mut offset = 0u64;
    for _ in hashes {
        let remaining = total.saturating_sub(offset);
        if remaining == 0 {
            break;
        }
        offset += remaining.min(CHUNK_BYTES as u64);
    }
    offset
}

pub(crate) fn verify_prefix(
    source: &mut (impl Read + Seek),
    destination: &mut (impl Read + Seek),
    total: u64,
    h: &Handle,
    upload: bool,
    fast: bool,
    temp_size: Option<u64>,
) -> Result<(u64, Sha256)> {
    let checkpoint = h.checkpoint.lock().unwrap().clone();
    let hashes = checkpoint.hashes;
    let expected = verified_size(&hashes, total);
    let fast = fast
        && upload
        && checkpoint.verified_bytes > 0
        && checkpoint.verified_bytes == expected
        && temp_size.is_some_and(|size| size >= expected);
    let mut bytes = vec![0; CHUNK_BYTES];
    let mut other = vec![0; CHUNK_BYTES];
    let mut offset = 0;
    let mut whole = Sha256::new();
    let mut valid = 0;
    for hash in &hashes {
        h.check_cancelled()?;
        let count = (total - offset).min(CHUNK_BYTES as u64) as usize;
        if count == 0 {
            break;
        }
        if fast {
            source.read_exact(&mut other[..count])?;
            if block_hash(&other[..count]) != *hash {
                return Err(crate::operation_error::error(
                    "SOURCE_CHANGED",
                    "Upload source content changed",
                ));
            }
            whole.update(&other[..count]);
        } else {
            if upload {
                source.read_exact(&mut other[..count])?;
                anyhow::ensure!(
                    block_hash(&other[..count]) == *hash,
                    "SOURCE_CHANGED: Upload source content changed"
                );
            }
            match destination.read_exact(&mut bytes[..count]) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(error) => return Err(error.into()),
            }
            if block_hash(&bytes[..count]) != *hash {
                break;
            }
            whole.update(&bytes[..count]);
        }
        offset += count as u64;
        valid += 1;
    }
    {
        let mut checkpoint = h.checkpoint.lock().unwrap();
        checkpoint.hashes.truncate(valid);
        checkpoint.verified_bytes = offset;
        checkpoint.temp_size = temp_size;
    }
    source.seek(SeekFrom::Start(offset))?;
    h.resumed(offset);
    h.persist()?;
    Ok((offset, whole))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfers::testing_handle;
    use std::io::Cursor;

    #[test]
    fn corrupted_checkpoint_tail_rolls_back_to_last_verified_block() {
        let h = testing_handle();
        let data = vec![0x53; CHUNK_BYTES * 2 + 17];
        h.checkpoint.lock().unwrap().hashes = vec![
            block_hash(&data[..CHUNK_BYTES]),
            block_hash(&data[CHUNK_BYTES..CHUNK_BYTES * 2]),
        ];
        let mut retained = data.clone();
        retained[CHUNK_BYTES + 2] = 0;
        let (offset, _) = verify_prefix(
            &mut Cursor::new(data.clone()),
            &mut Cursor::new(retained),
            data.len() as u64,
            &h,
            true,
            false,
            None,
        )
        .unwrap();
        assert_eq!(offset, CHUNK_BYTES as u64);
        assert_eq!(h.checkpoint.lock().unwrap().hashes.len(), 1);
    }

    #[test]
    fn source_content_change_is_rejected_even_with_matching_metadata() {
        let h = testing_handle();
        h.checkpoint.lock().unwrap().hashes = vec![block_hash(b"old")];
        let error = verify_prefix(
            &mut Cursor::new(b"new"),
            &mut Cursor::new(b"old"),
            3,
            &h,
            true,
            false,
            None,
        )
        .unwrap_err();
        assert_eq!(crate::operation_error::error_code(&error), "SOURCE_CHANGED");
    }

    #[test]
    fn fast_resume_skips_destination_scan_but_keeps_source_validation() {
        let h = testing_handle();
        let data = vec![9; CHUNK_BYTES + 3];
        {
            let mut checkpoint = h.checkpoint.lock().unwrap();
            checkpoint.hashes = vec![block_hash(&data[..CHUNK_BYTES])];
            checkpoint.verified_bytes = CHUNK_BYTES as u64;
            checkpoint.temp_size = Some(CHUNK_BYTES as u64);
        }
        let (offset, _) = verify_prefix(
            &mut Cursor::new(data.clone()),
            &mut Cursor::new(vec![0; CHUNK_BYTES]),
            data.len() as u64,
            &h,
            true,
            true,
            Some(CHUNK_BYTES as u64),
        )
        .unwrap();
        assert_eq!(offset, CHUNK_BYTES as u64);
        let mut changed = data.clone();
        changed[0] = 1;
        let error = verify_prefix(
            &mut Cursor::new(changed),
            &mut Cursor::new(vec![0; CHUNK_BYTES]),
            data.len() as u64,
            &h,
            true,
            true,
            Some(CHUNK_BYTES as u64),
        )
        .unwrap_err();
        assert_eq!(crate::operation_error::error_code(&error), "SOURCE_CHANGED");
    }

    #[test]
    fn sparse_source_sizes_above_four_gib_are_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large");
        let file = File::create(path).unwrap();
        file.set_len(5 * 1024 * 1024 * 1024 + 17).unwrap();
        assert_eq!(
            local_identity(&file).unwrap()["size"],
            5u64 * 1024 * 1024 * 1024 + 17
        );
    }
}
