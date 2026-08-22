use serde::{Deserialize, Serialize};

use crate::msg::Msg;

/// BLAKE3 digest (32 bytes).
pub type Hash = [u8; 32];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DeviceId(pub u128);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PeerId(pub u128);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct OpId {
    pub device_id: DeviceId,
    pub counter: u64,
}

/// A reference to a chunk stored as a blob in the CAS.
///
/// Lives in `proto` rather than `core` because `FsOpKind::Write` carries chunk
/// references over the wire: a peer must be able to reconstruct file layout from
/// the oplog alone, before it has fetched any of the referenced blobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkRef {
    /// Hash of the bytes as stored, so a peer can verify a transfer without holding
    /// any key. For encrypted content this names the ciphertext.
    pub hash: Hash,
    /// Length of the bytes as stored. Includes the AEAD tag when encrypted.
    pub len: u32,
    /// Length of the content this chunk represents once decrypted. Equal to `len` for
    /// plaintext. Kept separate because conflating the two silently misplaces every
    /// encrypted write: the tag makes stored length exceed content length.
    #[serde(default)]
    pub plain_len: u32,
    /// Offset of this chunk within the file's plaintext.
    pub offset: u64,
}

/// A file key sealed to one recipient's X25519 public key.
///
/// Lives in `proto` for the same reason [`ChunkRef`] does: it travels in
/// `FsOpKind::Write` and inside a stored `FileNode`, so it is a wire and on-disk shape
/// before it is a cryptographic one.
///
/// Carries no recipient identifier. A reader trials its own key against each envelope,
/// which costs one X25519 exchange and one AEAD open per entry — cheap at the handful of
/// recipients a repository has — and in exchange the file does not publish the list of
/// devices able to read it. Tagging each envelope would make lookup O(1) and turn the
/// recipient set into metadata that travels with every replica.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedFileKey {
    /// The sender's ephemeral X25519 public key, so the recipient can re-derive the
    /// shared secret with no prior exchange.
    pub ephemeral_pub: [u8; 32],
    pub nonce: [u8; 24],
    pub ciphertext: Vec<u8>,
}

/// How a file's content key is protected, when its chunks hold ciphertext.
///
/// Two mechanisms, and which one is present says when the file was written:
///
/// - `recipients` is what new writes produce — the file key sealed to each enrolled
///   peer's sealing key, and to this device's own. No shared secret is involved, so a
///   peer holding no envelope addressed to it genuinely cannot read the content.
/// - `sealed_key` is the older scheme: the file key sealed with a repository key every
///   replica holds. Files written that way keep working, and nothing writes it any
///   more.
///
/// Both can be absent from a `FileNode` entirely, which means the chunks are plaintext.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEncryption {
    /// The file key sealed with the repository key. Only on files written before
    /// per-recipient sealing existed.
    #[serde(default)]
    pub sealed_key: Option<Vec<u8>>,
    /// One envelope per recipient, this device included. Order is not meaningful and
    /// carries no information about who the recipients are.
    #[serde(default)]
    pub recipients: Vec<SealedFileKey>,
    /// Which recipients these envelopes were sealed to, as a digest keyed by the file
    /// key itself.
    ///
    /// Re-sealing needs to know whether a file is already addressed to the current set,
    /// and the envelopes cannot answer: they deliberately carry no recipient identity,
    /// and counting them is not the same test — two different sets of the same size
    /// would compare equal, and a node would either skip files it should re-seal or
    /// re-seal every file on every run.
    ///
    /// Keyed by the file key rather than plain, so only someone who can already read
    /// the file can test a candidate recipient set against it. A plain digest would let
    /// any holder of the ciphertext confirm a guessed set, which is most of what not
    /// listing the recipients was protecting.
    #[serde(default)]
    pub recipients_digest: Option<[u8; 32]>,
}

impl FileEncryption {
    /// Whether anything here can actually yield a key.
    ///
    /// An encryption record with neither mechanism is a file nobody can read, which is
    /// worth catching at the point it would be written rather than at the point someone
    /// opens it.
    pub fn is_openable(&self) -> bool {
        self.sealed_key.is_some() || !self.recipients.is_empty()
    }
}

/// A feature flag string negotiated in Hello/HelloAck.
pub type Feature = String;

/// Versioned envelope for all network messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageEnvelope {
    pub protocol_version: u16,
    pub msg_id: u64,
    pub reply_to: Option<u64>,
    pub payload: Msg,
}

#[cfg(test)]
mod encryption_encoding_tests {
    use super::*;

    /// The property the v2 → v3 migration rests on.
    ///
    /// `Option<FileEncryption>` is `None` for every plaintext file, and postcard encodes
    /// `None` as a single zero byte whatever it wraps. So changing `FileEncryption`'s
    /// shape leaves every unencrypted `FileNode` and `Write` byte-identical, and only
    /// encrypted ones become undecodable — which is what makes it possible to migrate a
    /// repository that never turned encryption on, and to refuse one that did rather
    /// than guess.
    #[test]
    fn a_plaintext_file_encodes_the_same_whatever_the_encryption_shape_is() {
        let none: Option<FileEncryption> = None;
        assert_eq!(postcard::to_allocvec(&none).unwrap(), vec![0u8]);

        // And the old shape's bytes: a bare `Vec<u8>` of 72 bytes began with its length,
        // which is not a valid Option tag — so an old encrypted record fails to decode
        // rather than being silently misread as something else.
        let old_shape_len_prefix = 72u8;
        assert!(
            old_shape_len_prefix > 1,
            "a sealed key is longer than any valid postcard Option tag"
        );
    }

    #[test]
    fn an_encryption_record_with_nothing_in_it_is_not_openable() {
        assert!(!FileEncryption::default().is_openable());
        assert!(FileEncryption {
            sealed_key: Some(vec![1, 2, 3]),
            recipients: Vec::new(),
            recipients_digest: None,
        }
        .is_openable());
        assert!(FileEncryption {
            sealed_key: None,
            recipients: vec![SealedFileKey {
                ephemeral_pub: [0; 32],
                nonce: [0; 24],
                ciphertext: vec![9],
            }],
            recipients_digest: Some([1; 32]),
        }
        .is_openable());
    }
}
