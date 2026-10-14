mod reader;
mod writer;
pub(crate) use reader::BlockReader;
pub(crate) use writer::BlockWriter;

pub(crate) const ARCHIVE_HEADER_SIZE: usize = 9;
pub(crate) const BLOCK_HEADER_SIZE: usize = 4;

#[allow(dead_code)] // используется из интеграционных тестов
pub(crate) fn triplicate(data: &[u8]) -> Vec<u8> {
    data.repeat(3)
}

pub(crate) fn detriplicate(data: &[u8], size: usize) -> Option<Vec<u8>> {
    if data.len() != size * 3 {
        return None;
    }
    let (a, rest) = data.split_at(size);
    let (b, c) = rest.split_at(size);
    let mut result = Vec::with_capacity(size);
    for i in 0..size {
        if a[i] == b[i] || a[i] == c[i] {
            result.push(a[i]);
        } else if b[i] == c[i] {
            result.push(b[i]);
        } else {
            return None;
        }
    }
    Some(result)
}

const SIGNATURE: &[u8; 8] = b"CLARSv03";

#[derive(Debug, Default)]
pub(crate) struct BlockStats {
    pub options: crate::archive::EncodingOptions,
    pub compressed: u64,
    pub stored: u64,
    pub corrected_shards: u64,
}
