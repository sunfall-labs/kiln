//! Length-framed wire contract for the process host capability.
//!
//! The C seam must not split command arguments on a delimiter: NUL is not a valid OS argument,
//! and delimiter framing makes malformed input look like a different command. This format is
//! deliberately boring so the provider and generated C can implement the same parser.

const VERSION: u32 = 1;
const HEADER: usize = 12;
const MAX_FIELD: usize = 16 * 1024 * 1024;
const MAX_ARGS: usize = 4096;
const NONE: u64 = u64::MAX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessWireRequest {
    pub command: Vec<u8>,
    pub args: Vec<Vec<u8>>,
    pub stdin: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessWireError {
    Truncated { field: &'static str },
    Version(u32),
    TooManyArgs(usize),
    FieldTooLarge { field: &'static str, length: u64 },
    EmbeddedNul { field: &'static str },
    TrailingBytes(usize),
}

fn get_u32(input: &[u8], at: &mut usize, field: &'static str) -> Result<u32, ProcessWireError> {
    let end = at
        .checked_add(4)
        .ok_or(ProcessWireError::Truncated { field })?;
    let bytes = input
        .get(*at..end)
        .ok_or(ProcessWireError::Truncated { field })?;
    *at = end;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

fn get_u64(input: &[u8], at: &mut usize, field: &'static str) -> Result<u64, ProcessWireError> {
    let end = at
        .checked_add(8)
        .ok_or(ProcessWireError::Truncated { field })?;
    let bytes = input
        .get(*at..end)
        .ok_or(ProcessWireError::Truncated { field })?;
    *at = end;
    Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
}

fn field(
    input: &[u8],
    at: &mut usize,
    length: u64,
    name: &'static str,
) -> Result<Vec<u8>, ProcessWireError> {
    if length > MAX_FIELD as u64 {
        return Err(ProcessWireError::FieldTooLarge {
            field: name,
            length,
        });
    }
    let length = length as usize;
    let end = at
        .checked_add(length)
        .ok_or(ProcessWireError::Truncated { field: name })?;
    let value = input
        .get(*at..end)
        .ok_or(ProcessWireError::Truncated { field: name })?;
    *at = end;
    if value.contains(&0) {
        return Err(ProcessWireError::EmbeddedNul { field: name });
    }
    Ok(value.to_vec())
}

pub fn encode(request: &ProcessWireRequest) -> Result<Vec<u8>, ProcessWireError> {
    if request.args.len() > MAX_ARGS {
        return Err(ProcessWireError::TooManyArgs(request.args.len()));
    }
    let mut out = Vec::with_capacity(HEADER);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(request.args.len() as u32).to_le_bytes());
    out.extend_from_slice(&(request.stdin.as_ref().map_or(NONE, |v| v.len() as u64)).to_le_bytes());
    for (name, value) in [
        ("command", &request.command),
        ("stdin", &request.stdin.clone().unwrap_or_default()),
    ] {
        if name == "stdin" && request.stdin.is_none() {
            continue;
        }
        if value.len() > MAX_FIELD {
            return Err(ProcessWireError::FieldTooLarge {
                field: name,
                length: value.len() as u64,
            });
        }
        if value.contains(&0) {
            return Err(ProcessWireError::EmbeddedNul { field: name });
        }
    }
    if request.command.len() > MAX_FIELD {
        return Err(ProcessWireError::FieldTooLarge {
            field: "command",
            length: request.command.len() as u64,
        });
    }
    out.extend_from_slice(&(request.command.len() as u64).to_le_bytes());
    out.extend_from_slice(&request.command);
    for arg in &request.args {
        if arg.len() > MAX_FIELD {
            return Err(ProcessWireError::FieldTooLarge {
                field: "argument",
                length: arg.len() as u64,
            });
        }
        if arg.contains(&0) {
            return Err(ProcessWireError::EmbeddedNul { field: "argument" });
        }
        out.extend_from_slice(&(arg.len() as u64).to_le_bytes());
        out.extend_from_slice(arg);
    }
    if let Some(stdin) = &request.stdin {
        out.extend_from_slice(stdin);
    }
    Ok(out)
}

pub fn decode(input: &[u8]) -> Result<ProcessWireRequest, ProcessWireError> {
    let mut at = 0;
    let version = get_u32(input, &mut at, "version")?;
    if version != VERSION {
        return Err(ProcessWireError::Version(version));
    }
    let argc = get_u32(input, &mut at, "argument count")? as usize;
    if argc > MAX_ARGS {
        return Err(ProcessWireError::TooManyArgs(argc));
    }
    let stdin_len = get_u64(input, &mut at, "stdin length")?;
    let command_len = get_u64(input, &mut at, "command length")?;
    let command = field(input, &mut at, command_len, "command")?;
    let mut args = Vec::with_capacity(argc);
    for _ in 0..argc {
        let length = get_u64(input, &mut at, "argument length")?;
        args.push(field(input, &mut at, length, "argument")?);
    }
    let stdin = if stdin_len == NONE {
        None
    } else {
        Some(field(input, &mut at, stdin_len, "stdin")?)
    };
    if at != input.len() {
        return Err(ProcessWireError::TrailingBytes(input.len() - at));
    }
    Ok(ProcessWireRequest {
        command,
        args,
        stdin,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_lengths_and_empty_stdin() {
        let request = ProcessWireRequest {
            command: b"printf".to_vec(),
            args: vec![b"%s".to_vec(), b"ok".to_vec()],
            stdin: Some(Vec::new()),
        };
        assert_eq!(decode(&encode(&request).unwrap()).unwrap(), request);
    }

    #[test]
    fn rejects_nul_and_truncation_and_trailing_bytes() {
        let nul = ProcessWireRequest {
            command: b"sh\0".to_vec(),
            args: Vec::new(),
            stdin: None,
        };
        assert!(matches!(
            encode(&nul),
            Err(ProcessWireError::EmbeddedNul { field: "command" })
        ));
        let bytes = encode(&ProcessWireRequest {
            command: b"echo".to_vec(),
            args: Vec::new(),
            stdin: None,
        })
        .unwrap();
        assert!(matches!(
            decode(&bytes[..bytes.len() - 1]),
            Err(ProcessWireError::Truncated { .. })
        ));
        let mut extra = bytes;
        extra.push(1);
        assert!(matches!(
            decode(&extra),
            Err(ProcessWireError::TrailingBytes(1))
        ));
    }

    #[test]
    fn rejects_bad_version_and_oversized_fields() {
        let mut bytes = vec![0; HEADER];
        bytes[..4].copy_from_slice(&2u32.to_le_bytes());
        assert_eq!(decode(&bytes), Err(ProcessWireError::Version(2)));
        let request = ProcessWireRequest {
            command: vec![b'x'; MAX_FIELD + 1],
            args: Vec::new(),
            stdin: None,
        };
        assert!(matches!(
            encode(&request),
            Err(ProcessWireError::FieldTooLarge {
                field: "command",
                ..
            })
        ));
    }
}
