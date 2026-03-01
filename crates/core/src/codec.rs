use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Serialize};

use crate::object::Object;

/// Canonical deterministic encoding for objects.
/// Use postcard to keep encoding stable and compact.
///
/// IMPORTANT: Any change to object structure must bump object version
/// (see ObjectHeader.version).
pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    postcard::to_stdvec(value).context("postcard encode")
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    postcard::from_bytes(bytes).context("postcard decode")
}

pub fn encode_object(value: &Object) -> Result<Vec<u8>> {
    encode(&value.canonicalized())
}
