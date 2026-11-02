use std::fmt;

/// Every failure has a stable machine-readable code (used over the C ABI and in tests) and a
/// human-readable detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Signature envelope malformed or too large.
    SigEnvelope(String),
    /// No trusted key produced a valid signature.
    BadSignature,
    /// Certificate signature invalid, or the cert is outside its validity window.
    BadCert(String),
    /// Manifest bytes are not a valid manifest.
    Manifest(String),
    /// Manifest is for another app id.
    WrongApp { expected: String, got: String },
    /// Manifest is for another channel.
    WrongChannel { expected: String, got: String },
    /// `expires` is in the past (freeze attack or stale mirror).
    Expired { expires: i64, now: i64 },
    /// `sequence` went down (rollback attack).
    SequenceRollback { last: u64, got: u64 },
    /// Same `sequence` but different bytes.
    SequenceReuse(u64),
    /// Blob or file hash did not match.
    HashMismatch { expected: String, got: String },
    /// Blob size did not match.
    SizeMismatch { expected: u64, got: u64 },
    /// A path tried to escape its directory, or was otherwise unsafe.
    UnsafePath(String),
    /// Payload descriptor invalid.
    Payload(String),
    /// Patch could not be applied.
    Patch(String),
    /// Archive problem (corrupt, bomb, symlink).
    Archive(String),
    /// Key or PEM problem.
    Key(String),
    /// State file problem or invalid transition.
    State(String),
    /// Caller misuse (unknown fetch key, nothing to stage...).
    Usage(String),
    /// Filesystem error.
    Io(String),
    /// Network error (transport feature only).
    Net(String),
    /// Not enough storage.
    NoSpace { needed: u64, available: u64 },
}

impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Error::SigEnvelope(_) => "sig_envelope",
            Error::BadSignature => "bad_signature",
            Error::BadCert(_) => "bad_cert",
            Error::Manifest(_) => "manifest",
            Error::WrongApp { .. } => "wrong_app",
            Error::WrongChannel { .. } => "wrong_channel",
            Error::Expired { .. } => "expired",
            Error::SequenceRollback { .. } => "sequence_rollback",
            Error::SequenceReuse(_) => "sequence_reuse",
            Error::HashMismatch { .. } => "hash_mismatch",
            Error::SizeMismatch { .. } => "size_mismatch",
            Error::UnsafePath(_) => "unsafe_path",
            Error::Payload(_) => "payload",
            Error::Patch(_) => "patch",
            Error::Archive(_) => "archive",
            Error::Key(_) => "key",
            Error::State(_) => "state",
            Error::Usage(_) => "usage",
            Error::Io(_) => "io",
            Error::Net(_) => "net",
            Error::NoSpace { .. } => "no_space",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::SigEnvelope(s) => write!(f, "signature envelope: {s}"),
            Error::BadSignature => write!(f, "signature does not verify under any trusted key"),
            Error::BadCert(s) => write!(f, "release key certificate: {s}"),
            Error::Manifest(s) => write!(f, "manifest: {s}"),
            Error::WrongApp { expected, got } => write!(f, "manifest is for app {got}, expected {expected}"),
            Error::WrongChannel { expected, got } => {
                write!(f, "manifest is for channel {got}, expected {expected}")
            }
            Error::Expired { expires, now } => write!(f, "manifest expired at {expires} (now {now})"),
            Error::SequenceRollback { last, got } => {
                write!(f, "manifest sequence {got} is lower than last accepted {last}")
            }
            Error::SequenceReuse(s) => write!(f, "manifest sequence {s} reused with different content"),
            Error::HashMismatch { expected, got } => write!(f, "sha256 mismatch: expected {expected}, got {got}"),
            Error::SizeMismatch { expected, got } => write!(f, "size mismatch: expected {expected}, got {got}"),
            Error::UnsafePath(s) => write!(f, "unsafe path: {s}"),
            Error::Payload(s) => write!(f, "payload descriptor: {s}"),
            Error::Patch(s) => write!(f, "patch: {s}"),
            Error::Archive(s) => write!(f, "archive: {s}"),
            Error::Key(s) => write!(f, "key: {s}"),
            Error::State(s) => write!(f, "state: {s}"),
            Error::Usage(s) => write!(f, "usage: {s}"),
            Error::Io(s) => write!(f, "io: {s}"),
            Error::Net(s) => write!(f, "network: {s}"),
            Error::NoSpace { needed, available } => {
                write!(f, "not enough space: need {needed} bytes, {available} available")
            }
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Attach a path to an io error.
pub(crate) fn io_at<T>(r: std::io::Result<T>, what: &str, path: &std::path::Path) -> Result<T> {
    r.map_err(|e| Error::Io(format!("{what} {}: {e}", path.display())))
}
