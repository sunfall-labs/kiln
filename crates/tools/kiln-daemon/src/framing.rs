//! Newline-delimited JSON framing.
//!
//! One JSON value per line, in both directions. Chosen over a length-prefixed or binary frame
//! because it is trivially inspectable - `nc -U <socket>` is a working client - which matters
//! far more at this stage than the handful of microseconds a binary encoding would save. Serde
//! never emits a raw newline inside a value, so a line break is an unambiguous frame boundary.

use std::io::{self, BufRead, ErrorKind, Read, Write};

/// Largest single message either side will buffer, in bytes.
///
/// Not a limit on real inputs - `SetSource` legitimately carries a whole file - but a bound on
/// what a peer that never sends a newline can make us allocate. A connection that exceeds it is
/// closed, because there is no way to resynchronise the frame stream.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

/// Read one frame, returning `None` at end of stream.
///
/// A trailing `\r` is tolerated so a client written against a CRLF-flavoured tool still works;
/// JSON itself never needs it.
pub fn read_frame<R: BufRead>(reader: &mut R, max: usize) -> io::Result<Option<String>> {
    let mut buf = Vec::new();
    // `max + 1` so a frame of exactly `max` bytes plus its newline still fits, while one byte
    // over is observable as "no newline anywhere in the window".
    let mut window = (&mut *reader).take(max as u64 + 1);
    let read = window.read_until(b'\n', &mut buf)?;

    if read == 0 {
        return Ok(None);
    }
    if buf.last() != Some(&b'\n') {
        if read > max {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!("message exceeds the {max} byte limit"),
            ));
        }
        // End of stream part-way through a frame: the peer died mid-write. There is no complete
        // message to act on, so treat it as a disconnect.
        return Ok(None);
    }

    buf.pop();
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    String::from_utf8(buf)
        .map(Some)
        .map_err(|e| io::Error::new(ErrorKind::InvalidData, e))
}

/// Write one already-encoded JSON value as a frame.
///
/// Callers encode to a `String` first (`serde_json::to_string`) so that a serialisation failure
/// cannot leave half a frame on the wire and desynchronise the peer.
pub fn write_frame<W: Write>(writer: &mut W, json: &str) -> io::Result<()> {
    debug_assert!(
        !json.contains('\n'),
        "a frame body must not contain a newline"
    );
    let mut line = String::with_capacity(json.len() + 1);
    line.push_str(json);
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_all(input: &[u8], max: usize) -> Vec<io::Result<Option<String>>> {
        let mut reader = io::BufReader::new(input);
        let mut out = Vec::new();
        loop {
            let frame = read_frame(&mut reader, max);
            let stop = !matches!(frame, Ok(Some(_)));
            out.push(frame);
            if stop {
                return out;
            }
        }
    }

    #[test]
    fn splits_on_newlines_and_tolerates_crlf() {
        let frames = read_all(b"{\"a\":1}\n{\"b\":2}\r\n", 64);
        let values: Vec<_> = frames
            .iter()
            .filter_map(|f| f.as_ref().ok().and_then(|o| o.clone()))
            .collect();
        assert_eq!(
            values,
            vec!["{\"a\":1}".to_string(), "{\"b\":2}".to_string()]
        );
        assert!(matches!(frames.last(), Some(Ok(None))), "clean EOF");
    }

    #[test]
    fn oversized_frame_is_an_error_not_an_allocation() {
        let mut input = vec![b'x'; 32];
        input.push(b'\n');
        let err = read_frame(&mut io::BufReader::new(&input[..]), 8)
            .expect_err("a 32 byte frame must not pass an 8 byte limit");
        assert_eq!(err.kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn frame_of_exactly_the_limit_is_accepted() {
        let mut input = vec![b'x'; 8];
        input.push(b'\n');
        let frame = read_frame(&mut io::BufReader::new(&input[..]), 8).expect("within limit");
        assert_eq!(frame.as_deref(), Some("xxxxxxxx"));
    }

    #[test]
    fn truncated_final_frame_reads_as_disconnect() {
        // A peer that dies mid-write leaves bytes with no newline. That is not a request.
        let frame = read_frame(&mut io::BufReader::new(&b"{\"a\":"[..]), 64).expect("not an error");
        assert_eq!(frame, None);
    }
}
