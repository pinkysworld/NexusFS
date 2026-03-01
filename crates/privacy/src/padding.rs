use serde::{Deserialize, Serialize};

/// Size padding policy.
///
/// This is a practical DP-adjacent primitive:
/// - pad file sizes to buckets to reduce exact size leakage
///
/// It does NOT hide access patterns by itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PaddingPolicy {
    None,
    PowerOfTwo,
    FixedBucket { bucket_bytes: u64 },
}

pub fn padded_size(policy: &PaddingPolicy, size: u64) -> u64 {
    match policy {
        PaddingPolicy::None => size,
        PaddingPolicy::PowerOfTwo => {
            let mut b = 1u64;
            while b < size {
                b <<= 1;
            }
            b
        }
        PaddingPolicy::FixedBucket { bucket_bytes } => {
            if *bucket_bytes == 0 {
                return size;
            }
            let rem = size % bucket_bytes;
            if rem == 0 {
                size
            } else {
                size + (bucket_bytes - rem)
            }
        }
    }
}
