pub mod archive;
mod block;
mod ecc;
mod lz77;
mod parallel;
mod rans;
mod varint;

use std::io;

const BLOCK_SIZE: usize = 64 * 1024;
pub const MAX_THREADS: usize = parallel::MAX_THREADS;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
