//! The one read failure that is not the server's fault.

/// This node holds the file and every envelope on it, and none of them are addressed to
/// it.
///
/// Typed rather than a plain message because the facades have to tell it apart from a
/// genuine failure: "you may not read this" is a 403, and returning 500 for it says the
/// daemon is broken when it is working exactly as designed. Matching on the text of an
/// error message would work until somebody rewords it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotARecipient {
    /// How many envelopes the file carries, none of which opened.
    pub envelopes: usize,
}

impl std::fmt::Display for NotARecipient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "this node is not a recipient of this file: {} envelope(s), none of which \
             open with this device's sealing key",
            self.envelopes
        )
    }
}

impl std::error::Error for NotARecipient {}

/// Whether `err` is a refusal to read rather than a failure to.
pub fn is_not_a_recipient(err: &anyhow::Error) -> bool {
    err.downcast_ref::<NotARecipient>().is_some()
}
