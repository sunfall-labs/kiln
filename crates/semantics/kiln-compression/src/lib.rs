//! Incremental gzip/deflate codecs shared by both JavaScript engines.
use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Gzip,
    Deflate,
    DeflateRaw,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Compress,
    Decompress,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidStream,
    AlreadyFinished,
}

#[derive(Debug)]
pub struct Codec {
    direction: Direction,
    compressor: Option<Compress>,
    decompressor: Option<Decompress>,
    finished: bool,
}
impl Codec {
    pub fn new(format: Format, direction: Direction) -> Self {
        let (compressor, decompressor) = match (format, direction) {
            (Format::Gzip, Direction::Compress) => {
                (Some(Compress::new_gzip(Compression::default(), 15)), None)
            }
            (Format::Gzip, Direction::Decompress) => (None, Some(Decompress::new_gzip(15))),
            (Format::Deflate, Direction::Compress) => {
                (Some(Compress::new(Compression::default(), true)), None)
            }
            (Format::Deflate, Direction::Decompress) => (None, Some(Decompress::new(true))),
            (Format::DeflateRaw, Direction::Compress) => {
                (Some(Compress::new(Compression::default(), false)), None)
            }
            (Format::DeflateRaw, Direction::Decompress) => (None, Some(Decompress::new(false))),
        };
        Self {
            direction,
            compressor,
            decompressor,
            finished: false,
        }
    }

    /// Consume one input chunk and return only output generated for this chunk. `finish` signals
    /// close and emits/validates the format trailer. The caller must call it exactly once.
    pub fn push(&mut self, input: &[u8], finish: bool) -> Result<Vec<u8>, Error> {
        if self.finished {
            return if finish && input.is_empty() {
                Ok(Vec::new())
            } else {
                Err(Error::AlreadyFinished)
            };
        }
        let mut output = Vec::new();
        let mut consumed_input = 0usize;
        loop {
            let mut chunk = [0u8; 64 * 1024];
            let before_in = match self.direction {
                Direction::Compress => self.compressor.as_ref().unwrap().total_in(),
                Direction::Decompress => self.decompressor.as_ref().unwrap().total_in(),
            };
            let before_out = match self.direction {
                Direction::Compress => self.compressor.as_ref().unwrap().total_out(),
                Direction::Decompress => self.decompressor.as_ref().unwrap().total_out(),
            };
            let status = match self.direction {
                Direction::Compress => self
                    .compressor
                    .as_mut()
                    .unwrap()
                    .compress(
                        &input[consumed_input..],
                        &mut chunk,
                        if finish {
                            FlushCompress::Finish
                        } else {
                            FlushCompress::None
                        },
                    )
                    .map_err(|_| Error::InvalidStream)?,
                Direction::Decompress => self
                    .decompressor
                    .as_mut()
                    .unwrap()
                    .decompress(
                        &input[consumed_input..],
                        &mut chunk,
                        if finish {
                            FlushDecompress::Finish
                        } else {
                            FlushDecompress::None
                        },
                    )
                    .map_err(|_| Error::InvalidStream)?,
            };
            let after_in = match self.direction {
                Direction::Compress => self.compressor.as_ref().unwrap().total_in(),
                Direction::Decompress => self.decompressor.as_ref().unwrap().total_in(),
            };
            let after_out = match self.direction {
                Direction::Compress => self.compressor.as_ref().unwrap().total_out(),
                Direction::Decompress => self.decompressor.as_ref().unwrap().total_out(),
            };
            let read = (after_in - before_in) as usize;
            let wrote = (after_out - before_out) as usize;
            consumed_input += read;
            output.extend_from_slice(&chunk[..wrote]);
            if status == Status::StreamEnd {
                // `flate2` reports where the first compressed member ended. The Web Streams
                // format accepts exactly one member; silently dropping bytes here would accept
                // trailing garbage and concatenated gzip members as if they were valid input.
                if consumed_input != input.len() {
                    return Err(Error::InvalidStream);
                }
                self.finished = true;
                break;
            }
            if consumed_input == input.len() && wrote == 0 {
                if finish && self.direction == Direction::Decompress {
                    return Err(Error::InvalidStream);
                }
                if finish && self.direction == Direction::Compress {
                    self.finished = true;
                }
                break;
            }
            if read == 0 && wrote == 0 {
                break;
            }
            if consumed_input == input.len() && !finish {
                break;
            }
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trips_across_chunk_boundaries_in_all_formats() {
        let data = b"streamed bytes across boundaries".repeat(10000);
        for format in [Format::Gzip, Format::Deflate, Format::DeflateRaw] {
            let mut encoder = Codec::new(format, Direction::Compress);
            let mut encoded = Vec::new();
            for part in data.chunks(37) {
                encoded.extend(encoder.push(part, false).unwrap());
            }
            encoded.extend(encoder.push(&[], true).unwrap());
            let mut decoder = Codec::new(format, Direction::Decompress);
            let mut decoded = Vec::new();
            for part in encoded.chunks(19) {
                decoded.extend(decoder.push(part, false).unwrap());
            }
            decoded.extend(decoder.push(&[], true).unwrap());
            assert_eq!(decoded, data);
        }
    }

    #[test]
    fn decompression_rejects_truncated_trailing_and_concatenated_streams() {
        for format in [Format::Gzip, Format::Deflate, Format::DeflateRaw] {
            let mut encoder = Codec::new(format, Direction::Compress);
            let mut encoded = encoder.push(b"payload", false).unwrap();
            encoded.extend(encoder.push(&[], true).unwrap());

            let mut truncated = Codec::new(format, Direction::Decompress);
            assert!(truncated.push(&encoded[..encoded.len() - 1], false).is_ok());
            assert_eq!(truncated.push(&[], true), Err(Error::InvalidStream));

            let mut trailing = Codec::new(format, Direction::Decompress);
            let mut with_junk = encoded.clone();
            with_junk.extend_from_slice(b"tail");
            assert_eq!(trailing.push(&with_junk, false), Err(Error::InvalidStream));

            let mut split_trailing = Codec::new(format, Direction::Decompress);
            split_trailing.push(&encoded, false).unwrap();
            assert_eq!(
                split_trailing.push(b"tail", false),
                Err(Error::AlreadyFinished)
            );

            if format == Format::Gzip {
                let mut concatenated = encoded.clone();
                concatenated.extend_from_slice(&encoded);
                let mut decoder = Codec::new(format, Direction::Decompress);
                assert_eq!(
                    decoder.push(&concatenated, false),
                    Err(Error::InvalidStream)
                );
            }
        }
    }
}
