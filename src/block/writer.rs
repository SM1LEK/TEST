use std::io::{self, Write};

use super::{BLOCK_HEADER_SIZE, SIGNATURE, triplicate};
use crate::{BLOCK_SIZE, ecc, lz77, parallel::Executor, rans};

pub(crate) struct BlockWriter<W: Write> {
    output: W,
    options: crate::archive::EncodingOptions,
    buffer: Vec<u8>,
    pending: Vec<Vec<u8>>,
    executor: Executor,
}

fn encode_block(raw: Vec<u8>, options: crate::archive::EncodingOptions) -> io::Result<Vec<u8>> {
    let raw_size = raw.len();
    let compressed = match (options.lz77, options.rans) {
        (false, false) => None,
        (false, true) => Some(rans::encode(&raw)?),
        (true, false) => Some(lz77::encode(&raw)?),
        (true, true) => {
            let lz77 = lz77::encode(&raw)?;
            Some(rans::encode(&lz77)?)
        }
    };
    let stored = match compressed {
        Some(compressed) if compressed.len() < raw_size => compressed,
        _ => raw,
    };
    // Неравенство размеров служит признаком сжатого блока в формате.
    let raw_bytes = ((raw_size - 1) as u16).to_le_bytes();
    let stored_bytes = ((stored.len() - 1) as u16).to_le_bytes();
    let header: [u8; BLOCK_HEADER_SIZE] =
        [raw_bytes[0], raw_bytes[1], stored_bytes[0], stored_bytes[1]];
    let mut output = if options.protected {
        ecc::encode(&header)
    } else {
        header.to_vec()
    };
    if options.protected {
        output.extend_from_slice(&ecc::encode(&stored));
    } else {
        output.extend_from_slice(&stored);
    }
    Ok(output)
}

impl<W: Write> BlockWriter<W> {
    pub fn with_threads(
        mut output: W,
        options: crate::archive::EncodingOptions,
        threads: Option<usize>,
    ) -> io::Result<Self> {
        let executor = Executor::new(threads)?;
        let mut header = SIGNATURE.to_vec();
        header.push(options.flags());
        output.write_all(&triplicate(&header))?;
        Ok(Self {
            output,
            options,
            buffer: Vec::with_capacity(BLOCK_SIZE),
            pending: Vec::new(),
            executor,
        })
    }

    fn queue_block(&mut self) -> io::Result<()> {
        if !self.buffer.is_empty() {
            self.pending.push(std::mem::replace(
                &mut self.buffer,
                Vec::with_capacity(BLOCK_SIZE),
            ));
        }
        if self.pending.len() == self.executor.batch_size() {
            self.flush_batch()?;
        }
        Ok(())
    }

    fn flush_batch(&mut self) -> io::Result<()> {
        let options = self.options;
        let batch: Vec<Vec<u8>> = std::mem::take(&mut self.pending);
        let packed = self.executor.map(batch, |raw| encode_block(raw, options))?;
        for block in packed {
            self.output.write_all(&block?)?;
        }
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<W> {
        self.flush()?;
        Ok(self.output)
    }
}

impl<W: Write> Write for BlockWriter<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let count = data.len().min(BLOCK_SIZE - self.buffer.len());
        self.buffer.extend_from_slice(&data[..count]);
        if self.buffer.len() == BLOCK_SIZE {
            self.queue_block()?;
        }
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.queue_block()?;
        self.flush_batch()?;
        self.output.flush()
    }
}
