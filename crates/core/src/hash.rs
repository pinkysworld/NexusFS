use anyhow::Result;
use nexusfs_storage::Hash;

use crate::codec::encode_object;
use crate::object::Object;

/// Hash arbitrary bytes with BLAKE3.
pub fn hash_bytes(data: &[u8]) -> Hash {
    blake3::hash(data).into()
}

pub fn hash_object(obj: &Object) -> Result<Hash> {
    Ok(hash_bytes(&encode_object(obj)?))
}
