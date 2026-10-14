use std::collections::VecDeque;
use std::io::{self, Read};

use super::{ARCHIVE_HEADER_SIZE, BLOCK_HEADER_SIZE, BlockStats, SIGNATURE, detriplicate};
use crate::{ecc, invalid, lz77, parallel::Executor, rans};

struct StoredBlock {
    original_size: usize,
    stored_size: usize,
    payload: Vec<u8>,
    corrected_shards: u64,
}

struct DecodedBlock {
    data: Vec<u8>,
    compressed: bool,
    corrected_shards: u64,
}

fn decode_block(
    block: StoredBlock,
    options: crate::archive::EncodingOptions,
) -> io::Result<DecodedBlock> {
    let (stored, payload_corrections) = if options.protected {
        ecc::decode(&block.payload)?
    } else {
        (block.payload, 0)
    };
    let compressed = block.stored_size < block.original_size;
    let mut data = stored;
    if compressed {
        // Этапы снимаются в порядке, обратном кодированию.
        if options.rans {
            data = rans::decode(&data)?;
        }
        if options.lz77 {
            data = lz77::decode(&data)?;
        }
    }
    if data.len() != block.original_size {
        return Err(invalid("Данные повреждены"));
    }
    Ok(DecodedBlock {
        data,
        compressed,
        corrected_shards: block.corrected_shards + payload_corrections,
    })
}

pub(crate) struct BlockReader<R: Read> {
    input: R,
    buffer: Vec<u8>,
    position: usize,
    stats: BlockStats,
    pending: VecDeque<io::Result<DecodedBlock>>,
    executor: Executor,
    eof: bool,
}

impl<R: Read> BlockReader<R> {
    pub fn with_threads(mut input: R, threads: Option<usize>) -> io::Result<Self> {
        let executor = Executor::new(threads)?;
        let mut stored_header = [0u8; ARCHIVE_HEADER_SIZE * 3];
        input.read_exact(&mut stored_header)?;
        let header = detriplicate(&stored_header, ARCHIVE_HEADER_SIZE)
            .ok_or_else(|| invalid("Повреждён заголовок архива"))?;
        if &header[..8] != SIGNATURE {
            return Err(invalid("Неизвестный формат"));
        }
        let options = crate::archive::EncodingOptions::from_flags(header[8])?;

        Ok(Self {
            input,
            buffer: Vec::new(),
            position: 0,
            stats: BlockStats {
                options,
                ..BlockStats::default()
            },
            pending: VecDeque::new(),
            executor,
            eof: false,
        })
    }

    fn read_stored(&mut self) -> io::Result<Option<StoredBlock>> {
        let protected = self.stats.options.protected;
        let header_len = if protected {
            ecc::encoded_size(BLOCK_HEADER_SIZE)
        } else {
            BLOCK_HEADER_SIZE
        };
        let mut header_buf = vec![0; header_len];

        loop {
            match self.input.read(&mut header_buf[..1]) {
                Ok(0) => return Ok(None),
                Ok(_) => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        self.input.read_exact(&mut header_buf[1..])?;

        let (header, corrected_shards) = if protected {
            let (header, corrected_shards) = ecc::decode(&header_buf)?;
            if header.len() != BLOCK_HEADER_SIZE {
                return Err(invalid("Неверный размер заголовка блока"));
            }
            (header, corrected_shards)
        } else {
            (header_buf, 0)
        };

        let original_size = usize::from(u16::from_le_bytes([header[0], header[1]])) + 1;
        let stored_size = usize::from(u16::from_le_bytes([header[2], header[3]])) + 1;
        if stored_size > original_size
            || (stored_size < original_size
                && ((!self.stats.options.lz77 && !self.stats.options.rans)
                    || (self.stats.options.rans && stored_size < rans::MIN_HEADER_SIZE)))
        {
            return Err(invalid("Неверный размер блока"));
        }

        let payload_len = if protected {
            ecc::encoded_size(stored_size)
        } else {
            stored_size
        };
        let mut payload = vec![0; payload_len];
        self.input.read_exact(&mut payload)?;
        Ok(Some(StoredBlock {
            original_size,
            stored_size,
            payload,
            corrected_shards,
        }))
    }

    fn load_batch(&mut self) -> io::Result<()> {
        let mut stored = Vec::new();
        let mut read_error = None;
        for _ in 0..self.executor.batch_size() {
            if self.eof {
                break;
            }
            match self.read_stored() {
                Ok(Some(block)) => stored.push(block),
                Ok(None) => self.eof = true,
                Err(error) => {
                    read_error = Some(error);
                    self.eof = true;
                    break;
                }
            }
        }
        let options = self.stats.options;
        // Результаты Rayon сохраняют исходный порядок блоков.
        self.pending = self
            .executor
            .map(stored, |block| decode_block(block, options))?
            .into();
        if let Some(error) = read_error {
            self.pending.push_back(Err(error));
        }
        Ok(())
    }

    fn load_block(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            self.load_batch()?;
        }
        let block = self.pending.pop_front().ok_or_else(|| {
            io::Error::new(io::ErrorKind::UnexpectedEof, "Обрыв блочного потока")
        })??;
        self.stats.corrected_shards += block.corrected_shards;
        if block.compressed {
            self.stats.compressed += 1;
        } else {
            self.stats.stored += 1;
        }
        self.buffer = block.data;
        self.position = 0;
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<BlockStats> {
        if self.position != self.buffer.len() || !self.pending.is_empty() {
            return Err(invalid("Лишние данные"));
        }
        if !self.eof {
            let mut extra = [0];
            loop {
                match self.input.read(&mut extra) {
                    Ok(0) => break,
                    Ok(_) => return Err(invalid("Лишние данные")),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(self.stats)
    }
}

impl<R: Read> Read for BlockReader<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if self.position == self.buffer.len() {
            self.load_block()?;
        }
        let count = output.len().min(self.buffer.len() - self.position);
        output[..count].copy_from_slice(&self.buffer[self.position..self.position + count]);
        self.position += count;
        Ok(count)
    }
}
