use std::fs;
use std::io;
use std::path::Path;
mod pack;
mod paths;
mod unpack;
pub use pack::create_with_options;
pub use unpack::{extract_with_threads, inspect_with_threads};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodingOptions {
    pub lz77: bool,
    pub rans: bool,
    pub protected: bool,
}

impl Default for EncodingOptions {
    fn default() -> Self {
        Self {
            lz77: true,
            rans: true,
            protected: false,
        }
    }
}

impl EncodingOptions {
    const LZ77_BIT: u8 = 1 << 0;
    const RANS_BIT: u8 = 1 << 1;
    const PROTECTED_BIT: u8 = 1 << 2;
    const ALL_BITS: u8 = Self::LZ77_BIT | Self::RANS_BIT | Self::PROTECTED_BIT;

    pub(crate) fn flags(self) -> u8 {
        let mut flags = 0;
        if self.lz77 {
            flags |= Self::LZ77_BIT;
        }
        if self.rans {
            flags |= Self::RANS_BIT;
        }
        if self.protected {
            flags |= Self::PROTECTED_BIT;
        }
        flags
    }

    pub(crate) fn from_flags(flags: u8) -> io::Result<Self> {
        if flags & !Self::ALL_BITS != 0 {
            return Err(crate::invalid("Неизвестные флаги кодирования"));
        }
        Ok(Self {
            lz77: flags & Self::LZ77_BIT != 0,
            rans: flags & Self::RANS_BIT != 0,
            protected: flags & Self::PROTECTED_BIT != 0,
        })
    }
}

#[derive(Debug)]

pub struct Entry {
    pub path: String,
    pub is_directory: bool,
    pub size: u64,
}

#[derive(Debug, Default)]
pub struct Statistics {
    pub files: u64,
    pub directories: u64,
    pub original_bytes: u64,
    pub archive_bytes: u64,
    pub protected: bool,
    pub lz77: bool,
    pub rans: bool,
    pub compressed_blocks: u64,
    pub stored_blocks: u64,
    pub corrected_shards: u64,
}

fn cleanup_error(path: &Path, directory: bool, original: io::Error) -> io::Error {
    let result = if directory {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    match result {
        Ok(()) => original,
        Err(error) => io::Error::new(
            original.kind(),
            format!(
                "{original}, очистка не завершена {}: {error}",
                path.display()
            ),
        ),
    }
}
