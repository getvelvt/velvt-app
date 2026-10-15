//! The binary layout every signed or authenticated vault structure uses
//! (design §5.3): a NUL-terminated tag, then big-endian integers and
//! fixed-length arrays, with `lp(x) = u16_be(len(x)) ‖ x` for the only
//! variable-length fields. A reader rejects a wrong tag, a short input and any
//! byte after the end, so one byte string has exactly one meaning.

/// Why a byte string is not a well-formed vault structure.
///
/// Carries no input bytes, so it is safe to log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FormatError {
    #[error("the bytes end before the structure does")]
    Truncated,
    #[error("the bytes do not start with the expected tag")]
    WrongTag,
    #[error("bytes follow the end of the structure")]
    TrailingBytes,
    #[error("a field is not valid: {0}")]
    InvalidField(&'static str),
}

pub(crate) struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    pub(crate) fn with_tag(tag: &[u8]) -> Self {
        debug_assert_eq!(tag.last(), Some(&0), "a vault tag is NUL-terminated");
        Self {
            bytes: tag.to_vec(),
        }
    }

    /// `lp(value)`. Only ever given a field whose type already bounds its
    /// length far below `u16::MAX` (`UserId`, `BlobName`).
    pub(crate) fn lp(&mut self, value: &[u8]) -> &mut Self {
        let length = u16::try_from(value.len()).expect("lp fields are length-checked by type");
        self.bytes.extend_from_slice(&length.to_be_bytes());
        self.bytes.extend_from_slice(value);
        self
    }

    pub(crate) fn u16(&mut self, value: u16) -> &mut Self {
        self.bytes.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub(crate) fn u64(&mut self, value: u64) -> &mut Self {
        self.bytes.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub(crate) fn i64(&mut self, value: i64) -> &mut Self {
        self.bytes.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub(crate) fn bytes(&mut self, value: &[u8]) -> &mut Self {
        self.bytes.extend_from_slice(value);
        self
    }

    pub(crate) fn finish(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.bytes)
    }
}

/// `pub` only so the sealed statement trait may name it; `wire` itself is
/// private to `vault`.
pub struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { rest: bytes }
    }

    pub(crate) fn tag(&mut self, tag: &[u8]) -> Result<(), FormatError> {
        match self.rest.strip_prefix(tag) {
            Some(rest) => {
                self.rest = rest;
                Ok(())
            }
            None => Err(FormatError::WrongTag),
        }
    }

    pub(crate) fn take(&mut self, length: usize) -> Result<&'a [u8], FormatError> {
        if self.rest.len() < length {
            return Err(FormatError::Truncated);
        }
        let (taken, rest) = self.rest.split_at(length);
        self.rest = rest;
        Ok(taken)
    }

    pub(crate) fn array<const N: usize>(&mut self) -> Result<[u8; N], FormatError> {
        let mut array = [0_u8; N];
        array.copy_from_slice(self.take(N)?);
        Ok(array)
    }

    pub(crate) fn u16(&mut self) -> Result<u16, FormatError> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    pub(crate) fn u64(&mut self) -> Result<u64, FormatError> {
        Ok(u64::from_be_bytes(self.array()?))
    }

    pub(crate) fn i64(&mut self) -> Result<i64, FormatError> {
        Ok(i64::from_be_bytes(self.array()?))
    }

    pub(crate) fn lp(&mut self) -> Result<&'a [u8], FormatError> {
        let length = self.u16()?;
        self.take(usize::from(length))
    }

    pub(crate) fn finish(self) -> Result<(), FormatError> {
        if self.rest.is_empty() {
            Ok(())
        } else {
            Err(FormatError::TrailingBytes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FormatError, Reader, Writer};

    #[test]
    fn a_written_structure_reads_back_and_nothing_else_does() {
        let bytes = Writer::with_tag(b"velvt-test-v1\0")
            .lp(b"user")
            .u16(0x0102)
            .u64(3)
            .i64(-4)
            .bytes(&[9; 4])
            .finish();

        let mut reader = Reader::new(&bytes);
        reader.tag(b"velvt-test-v1\0").unwrap();
        assert_eq!(reader.lp().unwrap(), b"user");
        assert_eq!(reader.u16().unwrap(), 0x0102);
        assert_eq!(reader.u64().unwrap(), 3);
        assert_eq!(reader.i64().unwrap(), -4);
        assert_eq!(reader.array::<4>().unwrap(), [9; 4]);
        reader.finish().unwrap();

        let mut longer = bytes.clone();
        longer.push(0);
        let mut reader = Reader::new(&longer);
        reader.take(bytes.len()).unwrap();
        assert_eq!(reader.finish(), Err(FormatError::TrailingBytes));

        let mut reader = Reader::new(&bytes);
        assert_eq!(reader.tag(b"velvt-other-v1\0"), Err(FormatError::WrongTag));
        let mut reader = Reader::new(&bytes[..bytes.len() - 1]);
        reader.take(bytes.len() - 4).unwrap();
        assert_eq!(reader.array::<4>(), Err(FormatError::Truncated));
    }

    #[test]
    fn a_length_prefix_longer_than_the_input_is_truncation_not_a_panic() {
        let mut reader = Reader::new(&[0xff, 0xff, 1, 2]);
        assert_eq!(reader.lp(), Err(FormatError::Truncated));
    }
}
