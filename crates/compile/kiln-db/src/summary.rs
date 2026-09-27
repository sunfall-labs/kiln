use kiln_digest::Digest;
use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord, Serialize, Deserialize,
)]
pub enum TypeGuess {
    #[default]
    Unknown,
    Number,
    Int,
    Boolean,
    String,
    Object,
    Callable,
    Undefined,
    Null,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FunctionSummary {
    pub parameter_types: Vec<TypeGuess>,
    pub return_type: TypeGuess,
    pub pure_or_unknown: bool,
    pub returned_sites: Vec<u32>,
    pub can_throw: bool,
    pub reads_arguments: bool,
}

impl FunctionSummary {
    pub fn is_pure(&self) -> bool {
        self.pure_or_unknown
    }

    pub fn digest(&self) -> Digest {
        let mut encoded = Vec::new();
        encoded.extend_from_slice(b"kiln-function-summary-v1\0");
        encoded.extend_from_slice(&(self.parameter_types.len() as u64).to_le_bytes());
        for parameter_type in &self.parameter_types {
            encoded.push(*parameter_type as u8);
        }
        encoded.push(self.return_type as u8);
        encoded.push(u8::from(self.pure_or_unknown));
        encoded.extend_from_slice(&(self.returned_sites.len() as u64).to_le_bytes());
        for site in &self.returned_sites {
            encoded.extend_from_slice(&site.to_le_bytes());
        }
        encoded.push(u8::from(self.can_throw));
        encoded.push(u8::from(self.reads_arguments));
        Digest::hash(&encoded)
    }
}

pub fn summaries_digest(summaries: &[FunctionSummary]) -> Digest {
    if summaries.is_empty() {
        return Digest::default();
    }
    let mut encoded = Vec::new();
    encoded.extend_from_slice(b"kiln-function-summaries-list-v1\0");
    encoded.extend_from_slice(&(summaries.len() as u64).to_le_bytes());
    for summary in summaries {
        encoded.extend_from_slice(summary.digest().as_bytes());
    }
    Digest::hash(&encoded)
}
