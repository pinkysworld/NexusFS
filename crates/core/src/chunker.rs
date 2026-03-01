use std::io::Read;

use anyhow::Result;
use nexusfs_storage::Hash;

use crate::hash::hash_bytes;
use crate::object::ChunkRef;

/// Fixed-size chunker (v0).
///
/// Later research tracks can replace this with content-defined chunking (CDC).
pub fn chunk_bytes(data: &[u8], chunk_size: usize) -> Vec<&[u8]> {
    data.chunks(chunk_size.max(1)).collect()
}

/// Produces (hash, bytes) pairs for chunks.
pub fn chunk_and_hash(data: &[u8], chunk_size: usize) -> Vec<(Hash, Vec<u8>)> {
    chunk_bytes(data, chunk_size)
        .into_iter()
        .map(|c| (hash_bytes(c), c.to_vec()))
        .collect()
}

pub fn chunk_reader<R: Read>(mut reader: R, chunk_size: usize) -> Result<Vec<Vec<u8>>> {
    let mut out = Vec::new();
    let mut buf = vec![0u8; chunk_size.max(1)];
    loop {
        let read = reader.read(&mut buf)?;
        if read == 0 {
            break;
        }
        out.push(buf[..read].to_vec());
    }
    Ok(out)
}

/// Helper: store all chunks using provided callback.
pub fn store_chunks<F>(data: &[u8], chunk_size: usize, mut put: F) -> Result<Vec<Hash>>
where
    F: FnMut(Hash, &[u8]) -> Result<()>,
{
    let mut out = Vec::new();
    for (h, bytes) in chunk_and_hash(data, chunk_size) {
        put(h, &bytes)?;
        out.push(h);
    }
    Ok(out)
}

pub fn store_chunks_with_refs<F>(
    data: &[u8],
    chunk_size: usize,
    mut put: F,
) -> Result<Vec<ChunkRef>>
where
    F: FnMut(Hash, &[u8]) -> Result<()>,
{
    let mut out = Vec::new();
    let mut offset = 0u64;
    for chunk in chunk_bytes(data, chunk_size.max(1)) {
        let hash = hash_bytes(chunk);
        put(hash, chunk)?;
        out.push(ChunkRef {
            hash,
            len: chunk.len() as u32,
            offset,
        });
        offset += chunk.len() as u64;
    }
    Ok(out)
}

pub fn store_reader_chunks_with_refs<R, F>(
    reader: R,
    chunk_size: usize,
    mut put: F,
) -> Result<Vec<ChunkRef>>
where
    R: Read,
    F: FnMut(Hash, &[u8]) -> Result<()>,
{
    let mut out = Vec::new();
    let mut offset = 0u64;
    for chunk in chunk_reader(reader, chunk_size)? {
        let hash = hash_bytes(&chunk);
        put(hash, &chunk)?;
        out.push(ChunkRef {
            hash,
            len: chunk.len() as u32,
            offset,
        });
        offset += chunk.len() as u64;
    }
    Ok(out)
}
