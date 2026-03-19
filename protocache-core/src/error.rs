#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CorruptionKind {
    Truncated,
    InvalidHeader,
    InvalidUtf8,
    IntegerOverflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadError {
    pub kind: CorruptionKind,
}

impl ReadError {
    pub const fn new(kind: CorruptionKind) -> Self {
        Self { kind }
    }
}

impl core::fmt::Display for ReadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}", self.kind)
    }
}

impl std::error::Error for ReadError {}

