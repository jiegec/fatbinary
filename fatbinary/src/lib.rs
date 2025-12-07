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
    /// 0x40 + optional fields
    header_size: u32,
    size: u64,
    compressed_size: u32,
    /// points to ptxas_options if available
    options_offset: u32,
    minor: u16,
    major: u16,
    arch: u32,
    identifier_offset: u32,
    identifier_len: u32,
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
    identifier: Option<String>,
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
                options_offset: 0x40,
                minor,
                major,
                arch: sm_arch,
                identifier_offset: 0,
                identifier_len: 0,
                flags: if is_64bit {
                    FATBINARY_FLAG_COMPILE_SIZE_64BIT
                } else {
                    0
                },
                zero: 0,
                decompressed_size: 0,
            },
            identifier: None,
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

    /// Get obj name
    pub fn get_identifier(&self) -> Option<&str> {
        self.identifier.as_deref()
    }

    /// Set the identifier (object name) for this entry
    pub fn set_identifier(&mut self, identifier: String) {
        self.identifier = Some(identifier);
    }

    /// Set the ptxas options for this entry (only valid for PTX entries)
    pub fn set_ptxas_options(&mut self, options: String) {
        self.ptxas_options = Some(options);
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
            let header_offset = reader.stream_position()?;
            let entry_header: FatBinaryEntryHeader = reader.read_le()?;
            let mut identifier = None;
            let mut ptxas_options = None;

            // handle ptxas options
            if entry_header.options_offset > 0 {
                reader.seek(SeekFrom::Start(
                    header_offset + entry_header.options_offset as u64,
                ))?;
                let ptxas_options_offset: u32 = reader.read_le()?;
                let ptxas_options_size: u32 = reader.read_le()?;

                // locate ptxas options
                if ptxas_options_offset != 0 {
                    reader.seek(SeekFrom::Start(header_offset + ptxas_options_offset as u64))?;
                    let mut ptxas_options_bytes = vec![0u8; ptxas_options_size as usize];
                    reader.read_exact(&mut ptxas_options_bytes)?;
                    ptxas_options = Some(String::from_utf8(ptxas_options_bytes)?);
                }
            }
            // handle object name
            if entry_header.identifier_offset > 0 {
                reader.seek(SeekFrom::Start(
                    header_offset + entry_header.identifier_offset as u64,
                ))?;
                let mut identifier_bytes = vec![0u8; entry_header.identifier_len as usize];
                reader.read_exact(&mut identifier_bytes)?;
                identifier = Some(String::from_utf8(identifier_bytes)?);
            }

            current_size += entry_header.header_size as u64;

            // seek to payload
            reader.seek(SeekFrom::Start(
                header_offset + entry_header.header_size as u64,
            ))?;
            let mut payload = vec![0; entry_header.size as usize];
            reader.read_exact(&mut payload[..])?;
            current_size += entry_header.size;

            entries.push(FatBinaryEntry {
                entry_header,
                identifier,
                ptxas_options,
                payload,
            })
        }

        let res = FatBinary { entries };
        Ok(res)
    }

    /// Write fatbinary to writer
    pub fn write<W: Write>(&self, mut writer: W) -> Result<(), FatBinaryError> {
        // Compute total size of all entries (including identifier and ptxas options)
        let mut total_size = 0u64;
        let mut entries_data = Vec::new();
        for entry in &self.entries {
            let identifier_bytes = entry
                .identifier
                .as_ref()
                .map(|s| s.as_bytes())
                .unwrap_or(&[]);
            let ptxas_options_bytes = entry
                .ptxas_options
                .as_ref()
                .map(|s| s.as_bytes())
                .unwrap_or(&[]);
            let identifier_len = identifier_bytes.len() as u32;
            let ptxas_options_len = ptxas_options_bytes.len() as u32;
            let header_size = entry.entry_header.header_size;
            let ptxas_options_offset = header_size + 8;
            let identifier_offset = ptxas_options_offset + ptxas_options_len;
            let header_total = header_size as u64
                + 8 // ptxas_options_offset + ptxas_options_size
                + ptxas_options_len as u64
                + identifier_len as u64;
            let entry_total = header_total + entry.entry_header.size;
            total_size += entry_total;
            entries_data.push((
                identifier_bytes,
                ptxas_options_bytes,
                identifier_len,
                ptxas_options_len,
                identifier_offset,
                ptxas_options_offset,
                header_total,
            ));
        }

        let header = FatBinaryHeader {
            magic: FAT_BINARY_MAGIC,
            version: 1,
            header_size: std::mem::size_of::<FatBinaryHeader>() as u16,
            size: total_size,
        };

        writer.write_all(&header.magic.to_le_bytes())?;
        writer.write_all(&header.version.to_le_bytes())?;
        writer.write_all(&header.header_size.to_le_bytes())?;
        writer.write_all(&header.size.to_le_bytes())?;

        for (entry, data) in self.entries.iter().zip(entries_data.iter()) {
            let (
                identifier_bytes,
                ptxas_options_bytes,
                identifier_len,
                ptxas_options_len,
                identifier_offset,
                ptxas_options_offset,
                full_header_size,
            ) = data;

            // Create a mutable copy of the header with updated identifier fields
            let mut header = entry.entry_header;
            header.header_size = *full_header_size as u32;
            header.identifier_offset = *identifier_offset;
            header.identifier_len = *identifier_len;

            // Write header fields
            writer.write_all(&header.kind.to_le_bytes())?;
            writer.write_all(&header.__unknown1.to_le_bytes())?;
            writer.write_all(&header.header_size.to_le_bytes())?;
            writer.write_all(&header.size.to_le_bytes())?;
            writer.write_all(&header.compressed_size.to_le_bytes())?;
            writer.write_all(&header.options_offset.to_le_bytes())?;
            writer.write_all(&header.minor.to_le_bytes())?;
            writer.write_all(&header.major.to_le_bytes())?;
            writer.write_all(&header.arch.to_le_bytes())?;
            writer.write_all(&header.identifier_offset.to_le_bytes())?;
            writer.write_all(&header.identifier_len.to_le_bytes())?;
            writer.write_all(&header.flags.to_le_bytes())?;
            writer.write_all(&header.zero.to_le_bytes())?;
            writer.write_all(&header.decompressed_size.to_le_bytes())?;

            // For PTX entries, options_offset is 0x40, which points to the start of extra header area.
            // Write ptxas_options_offset and ptxas_options_size as two u32s.
            writer.write_all(&ptxas_options_offset.to_le_bytes())?;
            writer.write_all(&ptxas_options_len.to_le_bytes())?;

            // Write ptxas options bytes
            writer.write_all(ptxas_options_bytes)?;

            // Write identifier bytes
            writer.write_all(identifier_bytes)?;

            // Write payload
            writer.write_all(&entry.payload)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::Cursor};

    use crate::{FatBinary, FatBinaryEntry};

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

    #[test]
    fn test_create_empty_fatbin() {
        let mut buffer = vec![];
        let cursor = Cursor::new(&mut buffer);
        let fatbin = FatBinary::new();
        fatbin.write(cursor).unwrap();

        let nvfatbin = nvfatbin_rs::Fatbin::new(&[]).unwrap();
        let nvfatbin_data = nvfatbin.to_vec().unwrap();
        assert_eq!(buffer, nvfatbin_data);
    }

    #[test]
    fn test_create_fatbin_with_ptx() {
        let mut nvfatbin = nvfatbin_rs::Fatbin::new(&["-compress=false"]).unwrap();
        let ptx = ".version 8.3\n.target sm_80\n.visible .entry test() {ret;}";
        nvfatbin.add_ptx(ptx, "80", "test.ptx", "").unwrap();
        let mut nvfatbin_data = nvfatbin.to_vec().unwrap();
        std::fs::write("test.fatbin", &nvfatbin_data).unwrap();

        let cursor = Cursor::new(&mut nvfatbin_data);
        let fatbin = FatBinary::read(cursor).unwrap();
        assert_eq!(
            String::from_utf8(fatbin.entries()[0].get_payload().to_vec()).unwrap(),
            "\n.target sm_80\n.visible .entry test() {ret;}\0\0\0\0"
        );
        assert_eq!(fatbin.entries()[0].get_identifier().unwrap(), "test.ptx");
    }

    #[test]
    fn test_write_identifier() {
        // Create a simple PTX entry
        let ptx = ".version 8.3\n.target sm_80\n.visible .entry test() {ret;}";
        let mut entry = FatBinaryEntry::new(false, 80, 8, 3, true, ptx.as_bytes());
        entry.set_identifier("mykernel.ptx".to_string());
        entry.set_ptxas_options("-O3".to_string());

        let mut fatbin = FatBinary::new();
        fatbin.entries_mut().push(entry);

        // Write to buffer
        let mut buffer = vec![];
        fatbin.write(Cursor::new(&mut buffer)).unwrap();

        // Read back
        let fatbin2 = FatBinary::read(Cursor::new(&buffer)).unwrap();
        let entries = fatbin2.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].get_identifier().unwrap(), "mykernel.ptx");
        assert_eq!(entries[0].get_ptxas_options().unwrap(), "-O3");
        assert_eq!(entries[0].get_sm_arch(), 80);
    }
}
