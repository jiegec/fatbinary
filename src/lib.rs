use binread::BinRead;
use binread::BinReaderExt;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum FatBinaryError {
    #[error("Invalid magic (expected {expected:?}, got {got:?})")]
    InvalidMagic { expected: u32, got: u32 },

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
pub struct FatBinaryHeader {
    pub magic: u32,
    pub version: u16,
    pub header_size: u16,
    pub size: u64,
}

#[repr(C, packed)]
#[derive(BinRead, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FatBinaryEntryHeader {
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

impl FatBinaryEntry {
    pub fn get_payload(&self) -> &[u8] {
        &self.payload
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
    header: FatBinaryHeader,
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

        let res = FatBinary { header, entries };
        Ok(res)
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
