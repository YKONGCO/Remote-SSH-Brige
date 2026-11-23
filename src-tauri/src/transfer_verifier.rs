use crate::{transfers::Handle, transfers::CHUNK_BYTES};
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, path::Path};

pub(crate) fn digest_reader(reader: &mut impl Read, h: &Handle) -> Result<String> {
    let mut hash = Sha256::new();
    let mut bytes = vec![0; CHUNK_BYTES];
    loop {
        h.check_cancelled()?;
        let count = reader.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub(crate) fn hash_local(path: &Path, h: &Handle) -> Result<String> {
    digest_reader(&mut File::open(path)?, h)
}
