use binread::BinRead;
use binread::BinReaderExt;
use std::borrow::Cow;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum FatBinaryError {
    #[error("Invalid magic (expected {expected:?}, got {got:?})")]
    InvalidMagic { expected: u32, got: u32 },
    #[error("Invalid version (expected {expected:?}, got {got:?})")]
    InvalidVersion { expected: u16, got: u16 },
    #[error("Invalid header size (expected {expected:?}, got {got:?})")]
    InvalidHeaderSize { expected: u16, got: u16 },

    #[error("Got binread::Error {source:?}")]
    Binread {
        #[from]
        source: binread::Error,
    },

    #[error("Got std::io::Error {source:?}")]
    Io {
        #[from]
        source: std::io::Error,
    },
}

// learned from https://github.com/n-eiling/cuda-fatbin-decompression/blob/9b194a9aa526b71131990ddd97ff5c41a273ace5/fatbin-decompress.h#L13
#[repr(C, packed)]
#[derive(BinRead, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct FatBinaryHeader {
    pub magic: u32,
    pub version: u16,
    pub header_size: u16,
    /// Size of payload beyond header
    pub size: u64,
}

#[repr(C, packed)]
#[derive(BinRead, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct FatBinaryEntryHeader {
    pub kind: u16,
    pub __unknown1: u16,
    pub header_size: u32,
    pub size: u64,
    pub compressed_size: u32,
    pub __unknown2: u32,
    pub minor: u16,
    pub major: u16,
    pub arch: u32,
    pub obj_name_offset: u32,
    pub obj_name_len: u32,
    pub flags: u64,
    pub zero: u64,
    pub decompressed_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FatBinaryEntry {
    entry_header: FatBinaryEntryHeader,
    payload: Vec<u8>,
}

// learned from https://github.com/n-eiling/cuda-fatbin-decompression/blob/9b194a9aa526b71131990ddd97ff5c41a273ace5/fatbin-decompress.c#L137
fn decompress(compressed: &[u8]) -> Vec<u8> {
    let mut res = vec![];

    let mut in_pos = 0;
    let mut next_non_compressed_len: usize;
    let mut next_compressed_len: usize;
    let mut back_offset: usize;

    while in_pos < compressed.len() {
        next_non_compressed_len = ((compressed[in_pos] & 0xf0) >> 4) as usize;
        next_compressed_len = (4 + (compressed[in_pos] & 0xf)) as usize;
        if next_non_compressed_len == 0xf {
            loop {
                in_pos += 1;
                next_non_compressed_len += compressed[in_pos] as usize;
                if compressed[in_pos] != 0xff {
                    break;
                }
            }
        }

        in_pos += 1;
        res.extend(&compressed[in_pos..(in_pos + next_non_compressed_len)]);

        in_pos += next_non_compressed_len;
        if in_pos >= compressed.len() {
            break;
        }
        back_offset = compressed[in_pos] as usize + ((compressed[in_pos + 1] as usize) << 8);
        in_pos += 2;

        if next_compressed_len == 0xf + 4 {
            loop {
                next_compressed_len += compressed[in_pos] as usize;
                in_pos += 1;
                if compressed[in_pos - 1] != 0xff {
                    break;
                }
            }
        }

        let res_len = res.len();
        for i in 0..next_compressed_len {
            res.push(res[res_len - back_offset + i]);
        }
    }

    res
}

impl FatBinaryEntry {
    pub fn get_payload(&self) -> &[u8] {
        if self.is_compressed() {
            &self.payload[..self.entry_header.compressed_size as usize]
        } else {
            &self.payload
        }
    }

    pub fn get_decompressed_payload(&self) -> Cow<'_, [u8]> {
        if self.is_compressed() {
            Cow::Owned(decompress(
                &self.payload[..self.entry_header.compressed_size as usize],
            ))
        } else {
            Cow::Borrowed(&self.payload)
        }
    }

    pub fn decompress(&mut self) {
        if self.is_compressed() {
            self.payload = decompress(&self.payload[..self.entry_header.compressed_size as usize]);
            self.entry_header.flags &= !0x2000; // clear compressed flag

            assert_eq!(
                self.payload.len(),
                self.entry_header.decompressed_size as usize
            );
            self.entry_header.size = self.entry_header.decompressed_size;
            self.entry_header.compressed_size = 0;
            self.entry_header.decompressed_size = 0;
        }
    }

    pub fn contains_elf(&self) -> bool {
        self.entry_header.kind == 2
    }

    pub fn get_sm_arch(&self) -> u32 {
        self.entry_header.arch
    }

    pub fn get_version_major(&self) -> u16 {
        self.entry_header.major
    }

    pub fn get_version_minor(&self) -> u16 {
        self.entry_header.minor
    }

    pub fn compile_size_is_64bit(&self) -> bool {
        (self.entry_header.flags & 0x10) != 0
    }

    pub fn is_compressed(&self) -> bool {
        (self.entry_header.flags & 0x2000) != 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FatBinary {
    entries: Vec<FatBinaryEntry>,
}

const FAT_BINARY_MAGIC: u32 = 0xBA55ED50;

impl FatBinary {
    pub fn get_entries(&self) -> &[FatBinaryEntry] {
        &self.entries
    }

    pub fn read<R: Read + Seek>(mut reader: R) -> Result<FatBinary, FatBinaryError> {
        let header: FatBinaryHeader = reader.read_le()?;

        if header.magic != FAT_BINARY_MAGIC {
            return Err(FatBinaryError::InvalidMagic {
                expected: FAT_BINARY_MAGIC,
                got: header.magic,
            });
        }

        if header.version != 1 {
            return Err(FatBinaryError::InvalidVersion {
                expected: 1,
                got: header.version,
            });
        }

        if header.header_size != std::mem::size_of::<FatBinaryHeader>() as u16 {
            return Err(FatBinaryError::InvalidHeaderSize {
                expected: std::mem::size_of::<FatBinaryHeader>() as u16,
                got: header.header_size,
            });
        }

        let mut entries = vec![];
        let mut current_size = 0;

        while current_size < header.size {
            let entry_header: FatBinaryEntryHeader = reader.read_le()?;

            // handle case when header size == 72
            if entry_header.header_size > std::mem::size_of::<FatBinaryEntryHeader>() as u32 {
                reader.seek(SeekFrom::Current(
                    entry_header.header_size as i64
                        - std::mem::size_of::<FatBinaryEntryHeader>() as i64,
                ))?;
            }
            current_size += entry_header.header_size as u64;

            let mut payload = vec![0; entry_header.size as usize];
            reader.read_exact(&mut payload[..])?;
            current_size += entry_header.size;

            entries.push(FatBinaryEntry {
                entry_header,
                payload,
            })
        }

        let res = FatBinary { entries };
        Ok(res)
    }

    pub fn write<W: Write>(mut writer: W) -> Result<(), FatBinaryError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        let result = 2 + 2;
        assert_eq!(result, 4);
    }
}
