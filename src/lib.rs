//! fatbinary crate: parse and manipulate fatbinary files
//!
//! You can use [FatBinary] struct to open or create fatbinary files. Fatbinary
//! contains multiple entries containing ELF or PTX files, and each entry can be
//! accessed via [FatBinaryEntry].
//!

use binread::BinRead;
use binread::BinReaderExt;
use std::borrow::Cow;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use thiserror::Error;

/// Errors from fatbinary crate
#[derive(Error, Debug)]
pub enum FatBinaryError {
    /// Got invalid magic number
    #[error("Invalid magic (expected {expected:?}, got {got:?})")]
    InvalidMagic { expected: u32, got: u32 },

    /// Got invalid fatbinary veresion
    #[error("Invalid version (expected {expected:?}, got {got:?})")]
    InvalidVersion { expected: u16, got: u16 },

    /// Got invalid header size
    #[error("Invalid header size (expected {expected:?}, got {got:?})")]
    InvalidHeaderSize { expected: u16, got: u16 },

    /// Got invalid offset
    #[error("Invalid offset (expected {expected:?}, got {got:?})")]
    InvalidOffset { expected: u32, got: u32 },

    /// Got error from binread crate
    #[error("Got binread::Error {source:?}")]
    Binread {
        #[from]
        source: binread::Error,
    },

    /// Got error from std::io module
    #[error("Got std::io::Error {source:?}")]
    Io {
        #[from]
        source: std::io::Error,
    },

    /// Got error std::string::FromUtf8Error
    #[error("Got std::string::FromUtf8Error {source:?}")]
    FromUtf8 {
        #[from]
        source: std::string::FromUtf8Error,
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

// learned from https://github.com/n-eiling/cuda-fatbin-decompression/blob/9b194a9aa526b71131990ddd97ff5c41a273ace5/fatbin-decompress.c#L22

const FATBINARY_FLAG_COMPILE_SIZE_64BIT: u64 = 0x00000001;
const FATBINARY_FLAG_DEBUG: u64 = 0x00000002;
const FATBINARY_FLAG_PRODUCER_CUDA: u64 = 0x00000004;
const FATBINARY_FLAG_PRODUCER_OPENCL: u64 = 0x00000008;
const FATBINARY_FLAG_HOST_LINUX: u64 = 0x00000010;
const FATBINARY_FLAG_HOST_MAC: u64 = 0x00000020;
const FATBINARY_FLAG_HOST_WINDOWS: u64 = 0x00000040;
const FATBINARY_FLAG_COMPRESSED: u64 = 0x00002000;

/// Host platform of [FatBinaryEntry]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Host {
    Linux,
    Mac,
    Windows,
    Unknown,
}

/// Producer of the [FatBinaryEntry]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Producer {
    CUDA,
    OpenCL,
    Unknown,
}

/// Header of an entry in fat binary
#[repr(C, packed)]
#[derive(BinRead, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FatBinaryEntryHeader {
    /// 0x02 if ELF, 0x01 if PTX
    kind: u16,
    /// 0x101
    __unknown1: u16,
    /// 0x40 if ELF, >=0x48 if PTX
    header_size: u32,
    size: u64,
    compressed_size: u32,
    /// 0x00 if ELF, 0x40 if PTX
    options_offset: u32,
    minor: u16,
    major: u16,
    arch: u32,
    obj_name_offset: u32,
    obj_name_len: u32,
    flags: u64,
    zero: u64,
    decompressed_size: u64,
    // additional 8 bytes here if PTX
    // ptxas_options_offset: u4,
    // ptxas_options_size: u4
}

/// A fatbinary entry
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FatBinaryEntry {
    entry_header: FatBinaryEntryHeader,
    ptxas_options: Option<String>,
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
    /// Create a new entry with autodetection
    pub fn new_auto<T: Into<Vec<u8>>>(sm_arch: u32, payload: T) -> Self {
        let payload: Vec<u8> = payload.into();

        // check ELF magic
        let is_elf = payload.starts_with(&[0x7f, 0x45, 0x4c, 0x46]);
        Self::new(is_elf, sm_arch, 0, 0, true, payload)
    }

    /// Create a new entry
    pub fn new<T: Into<Vec<u8>>>(
        is_elf: bool,
        sm_arch: u32,
        major: u16,
        minor: u16,
        is_64bit: bool,
        payload: T,
    ) -> Self {
        let payload: Vec<u8> = payload.into();
        Self {
            entry_header: FatBinaryEntryHeader {
                kind: if is_elf { 2 } else { 1 },
                __unknown1: 0x0101,
                header_size: 64,
                size: payload.len() as u64,
                compressed_size: 0,
                options_offset: if is_elf { 0x00 } else { 0x40 },
                minor,
                major,
                arch: sm_arch,
                obj_name_offset: 0,
                obj_name_len: 0,
                flags: if is_64bit {
                    FATBINARY_FLAG_COMPILE_SIZE_64BIT
                } else {
                    0
                },
                zero: 0,
                decompressed_size: 0,
            },
            ptxas_options: None,
            payload,
        }
    }
    /// Get (possibly compressed) payload contained in this entry
    pub fn get_payload(&self) -> &[u8] {
        if self.is_compressed() {
            &self.payload[..self.entry_header.compressed_size as usize]
        } else {
            &self.payload
        }
    }

    /// Get payload contained in this entry, decompress if it was compressed
    pub fn get_decompressed_payload(&self) -> Cow<'_, [u8]> {
        if self.is_compressed() {
            Cow::Owned(decompress(
                &self.payload[..self.entry_header.compressed_size as usize],
            ))
        } else {
            Cow::Borrowed(&self.payload)
        }
    }

    /// Replace the payload with decompressed data
    pub fn decompress(&mut self) {
        if self.is_compressed() {
            self.payload = decompress(&self.payload[..self.entry_header.compressed_size as usize]);
            self.entry_header.flags &= !FATBINARY_FLAG_COMPRESSED; // clear compressed flag

            assert_eq!(
                self.payload.len(),
                self.entry_header.decompressed_size as usize
            );
            self.entry_header.size = self.entry_header.decompressed_size;
            self.entry_header.compressed_size = 0;
            self.entry_header.decompressed_size = 0;
        }
    }

    /// Check if this entry contains ELF
    pub fn contains_elf(&self) -> bool {
        self.entry_header.kind == 2
    }

    /// Get CUDA SM architecture
    pub fn get_sm_arch(&self) -> u32 {
        self.entry_header.arch
    }

    /// Get major version
    pub fn get_version_major(&self) -> u16 {
        self.entry_header.major
    }

    /// Get minor version
    pub fn get_version_minor(&self) -> u16 {
        self.entry_header.minor
    }

    /// Check if compiled for 64 bit
    pub fn is_64bit(&self) -> bool {
        (self.entry_header.flags & FATBINARY_FLAG_COMPILE_SIZE_64BIT) != 0
    }

    /// Get compiled in/for which host
    pub fn host(&self) -> Host {
        if (self.entry_header.flags & FATBINARY_FLAG_HOST_LINUX) != 0 {
            Host::Linux
        } else if (self.entry_header.flags & FATBINARY_FLAG_HOST_MAC) != 0 {
            Host::Mac
        } else if (self.entry_header.flags & FATBINARY_FLAG_HOST_WINDOWS) != 0 {
            Host::Windows
        } else {
            Host::Unknown
        }
    }

    /// Get the producer of this entry
    pub fn producer(&self) -> Producer {
        if (self.entry_header.flags & FATBINARY_FLAG_PRODUCER_CUDA) != 0 {
            Producer::CUDA
        } else if (self.entry_header.flags & FATBINARY_FLAG_PRODUCER_OPENCL) != 0 {
            Producer::OpenCL
        } else {
            Producer::Unknown
        }
    }

    /// Check if payload is compressed
    pub fn is_compressed(&self) -> bool {
        (self.entry_header.flags & FATBINARY_FLAG_COMPRESSED) != 0
    }

    /// Check if debug info is contained
    pub fn has_debug_info(&self) -> bool {
        (self.entry_header.flags & FATBINARY_FLAG_DEBUG) != 0
    }

    /// Get header of this entry
    pub fn get_header(&self) -> &FatBinaryEntryHeader {
        &self.entry_header
    }

    /// Get ptxas options
    pub fn get_ptxas_options(&self) -> Option<&str> {
        self.ptxas_options.as_deref()
    }
}

/// A fatbinary file
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct FatBinary {
    entries: Vec<FatBinaryEntry>,
}

const FAT_BINARY_MAGIC: u32 = 0xBA55ED50;

impl FatBinary {
    /// Get entries contained in the fatbinary
    pub fn entries(&self) -> &Vec<FatBinaryEntry> {
        &self.entries
    }

    /// Get mutable entries contained in the fatbinary
    pub fn entries_mut(&mut self) -> &mut Vec<FatBinaryEntry> {
        &mut self.entries
    }

    /// Create a new empty fatbinary
    pub fn new() -> Self {
        Self { entries: vec![] }
    }

    /// Read fatbinary from reader
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

            // handle case when header size > 64 e.g. PTX
            let mut ptxas_options = None;
            if entry_header.header_size > std::mem::size_of::<FatBinaryEntryHeader>() as u32 {
                if entry_header.options_offset != 0x40 {
                    return Err(FatBinaryError::InvalidOffset {
                        expected: 0x40,
                        got: entry_header.options_offset,
                    });
                }
                let ptxas_options_offset: u32 = reader.read_le()?;
                let ptxas_options_size: u32 = reader.read_le()?;

                // locate ptxas options
                if ptxas_options_offset != 0 {
                    reader.seek(SeekFrom::Current(
                        (
                            ptxas_options_offset as usize
                        - std::mem::size_of::<FatBinaryEntryHeader>()
                        - std::mem::size_of::<u32>() // ptxas_options_offset
                        - std::mem::size_of::<u32>()
                            // ptxas_options_size
                        ) as i64,
                    ))?;
                    let mut ptxas_options_bytes = vec![0u8; ptxas_options_size as usize];
                    reader.read_exact(&mut ptxas_options_bytes)?;
                    ptxas_options = Some(String::from_utf8(ptxas_options_bytes)?);
                }

                // seek to payload
                reader.seek(SeekFrom::Current(
                    (
                        entry_header.header_size as usize
                        - std::mem::size_of::<FatBinaryEntryHeader>()
                        - std::mem::size_of::<u32>() // ptxas_options_offset
                        - std::mem::size_of::<u32>() // ptxas_options_size
                        - ptxas_options_size as usize
                        // ptxas_options
                    ) as i64,
                ))?;
            }
            current_size += entry_header.header_size as u64;

            let mut payload = vec![0; entry_header.size as usize];
            reader.read_exact(&mut payload[..])?;
            current_size += entry_header.size;

            entries.push(FatBinaryEntry {
                entry_header,
                ptxas_options,
                payload,
            })
        }

        let res = FatBinary { entries };
        Ok(res)
    }

    /// Wriet fatbinary to writer
    pub fn write<W: Write>(&self, mut writer: W) -> Result<(), FatBinaryError> {
        let payload_size = self
            .entries
            .iter()
            .map(|entry| entry.entry_header.header_size as u64 + entry.entry_header.size)
            .sum();
        let header = FatBinaryHeader {
            magic: FAT_BINARY_MAGIC,
            version: 1,
            header_size: std::mem::size_of::<FatBinaryHeader>() as u16,
            size: payload_size,
        };

        writer.write_all(&header.magic.to_le_bytes())?;
        writer.write_all(&header.version.to_le_bytes())?;
        writer.write_all(&header.header_size.to_le_bytes())?;
        writer.write_all(&header.size.to_le_bytes())?;

        for entry in &self.entries {
            writer.write_all(&entry.entry_header.kind.to_le_bytes())?;
            writer.write_all(&entry.entry_header.__unknown1.to_le_bytes())?;
            writer.write_all(&entry.entry_header.header_size.to_le_bytes())?;
            writer.write_all(&entry.entry_header.size.to_le_bytes())?;
            writer.write_all(&entry.entry_header.compressed_size.to_le_bytes())?;
            writer.write_all(&entry.entry_header.options_offset.to_le_bytes())?;
            writer.write_all(&entry.entry_header.minor.to_le_bytes())?;
            writer.write_all(&entry.entry_header.major.to_le_bytes())?;
            writer.write_all(&entry.entry_header.arch.to_le_bytes())?;
            writer.write_all(&entry.entry_header.obj_name_offset.to_le_bytes())?;
            writer.write_all(&entry.entry_header.obj_name_len.to_le_bytes())?;
            writer.write_all(&entry.entry_header.flags.to_le_bytes())?;
            writer.write_all(&entry.entry_header.zero.to_le_bytes())?;
            writer.write_all(&entry.entry_header.decompressed_size.to_le_bytes())?;

            if entry.entry_header.header_size > std::mem::size_of::<FatBinaryEntryHeader>() as u32 {
                let zeros = vec![
                    0u8;
                    entry.entry_header.header_size as usize
                        - std::mem::size_of::<FatBinaryEntryHeader>()
                ];
                writer.write_all(&zeros)?;
            }

            writer.write_all(&entry.payload)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;

    use crate::FatBinary;

    #[test]
    fn read_axpy_default() {
        let file = File::open("tests/axpy-default.fatbin").unwrap();
        let fatbin = FatBinary::read(file).unwrap();

        // has two entries
        let entries = fatbin.entries();
        assert_eq!(entries.len(), 2);

        // first is elf
        assert!(entries[0].contains_elf());
        // --cuda-gpu-arch=sm_70 is specified in build.sh
        assert_eq!(entries[0].get_sm_arch(), 70);

        // second is ptx
        assert!(!entries[1].contains_elf());
        // --cuda-gpu-arch=sm_70 is specified in build.sh
        assert_eq!(entries[1].get_sm_arch(), 70);

        // check if valid ptx
        let ptx_code = String::from_utf8(entries[1].get_decompressed_payload().to_vec()).unwrap();
        assert!(ptx_code.contains(".target sm_70"));
    }

    #[test]
    fn read_axpy_debug() {
        let file = File::open("tests/axpy-debug.fatbin").unwrap();
        let fatbin = FatBinary::read(file).unwrap();

        let entries = fatbin.entries();

        // first is elf
        assert!(entries[0].has_debug_info());

        // second is ptx
        assert!(entries[1].has_debug_info());
    }

    #[test]
    fn read_axpy_ptxas_options() {
        let file = File::open("tests/axpy-ptxas-options.fatbin").unwrap();
        let fatbin = FatBinary::read(file).unwrap();

        let entries = fatbin.entries();

        // second is ptx
        assert_eq!(entries[1].get_ptxas_options().unwrap().trim(), "-O3");
    }
}
