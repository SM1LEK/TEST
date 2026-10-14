use reed_solomon_erasure::galois_8::ReedSolomon;
use std::io;
use std::sync::OnceLock;

use crate::invalid;

const DATA_SHARDS: usize = 10;
const PARITY_SHARDS: usize = 2;
const TOTAL_SHARDS: usize = DATA_SHARDS + PARITY_SHARDS;

fn codec() -> &'static ReedSolomon {
    static CODEC: OnceLock<ReedSolomon> = OnceLock::new();
    // Параметры кодека постоянны, поэтому экземпляр безопасно переиспользуется.
    CODEC.get_or_init(|| ReedSolomon::new(DATA_SHARDS, PARITY_SHARDS).unwrap())
}

fn shard_size(size: usize) -> usize {
    (size + 4).div_ceil(DATA_SHARDS).max(1)
}

pub(crate) fn encoded_size(size: usize) -> usize {
    TOTAL_SHARDS * 4 + TOTAL_SHARDS * shard_size(size)
}

pub(crate) fn encode(data: &[u8]) -> Vec<u8> {
    let original_len = data.len() as u32;
    let protected_len = data.len() + 4;
    let shard_size = shard_size(data.len());

    let mut protected_data = Vec::with_capacity(protected_len);
    protected_data.extend_from_slice(&original_len.to_le_bytes());
    protected_data.extend_from_slice(data);

    let mut shards = vec![vec![0u8; shard_size]; TOTAL_SHARDS];
    for (index, chunk) in protected_data.chunks(shard_size).enumerate() {
        if index >= DATA_SHARDS {
            break;
        }
        shards[index][..chunk.len()].copy_from_slice(chunk);
    }

    codec().encode(&mut shards).unwrap();

    let mut output = Vec::with_capacity(TOTAL_SHARDS * 4 + TOTAL_SHARDS * shard_size);
    for shard in &shards {
        output.extend_from_slice(&crc32fast::hash(shard).to_le_bytes());
    }
    for shard in &shards {
        output.extend_from_slice(shard);
    }
    output
}

pub(crate) fn decode(input: &[u8]) -> io::Result<(Vec<u8>, u64)> {
    let crc_table_len = TOTAL_SHARDS * 4;
    if input.len() <= crc_table_len {
        return Err(invalid("Обрезанные данные RS"));
    }

    let body_len = input.len() - crc_table_len;
    if !body_len.is_multiple_of(TOTAL_SHARDS) {
        return Err(invalid("Некорректный размер данных RS"));
    }
    let shard_size = body_len / TOTAL_SHARDS;
    if shard_size == 0 {
        return Err(invalid("Нулевой размер фрагмента RS"));
    }

    let expected_crc: [u32; TOTAL_SHARDS] = std::array::from_fn(|i| {
        let pos = i * 4;
        u32::from_le_bytes(input[pos..pos + 4].try_into().unwrap())
    });

    let body = &input[crc_table_len..];
    let mut shards: Vec<Option<Vec<u8>>> = Vec::with_capacity(TOTAL_SHARDS);
    let mut repaired = 0u64;
    for (i, expected_crc) in expected_crc.into_iter().enumerate() {
        let start = i * shard_size;
        let end = start + shard_size;
        let shard = body[start..end].to_vec();
        if crc32fast::hash(&shard) == expected_crc {
            shards.push(Some(shard));
        } else {
            // Повреждённый CRC превращает фрагмент в стирание для Reed–Solomon.
            shards.push(None);
            repaired += 1;
        }
    }

    if repaired > PARITY_SHARDS as u64 {
        return Err(invalid("Слишком много повреждённых фрагментов"));
    }

    if repaired > 0 {
        codec()
            .reconstruct(&mut shards)
            .map_err(|_| invalid("RS не удалось восстановить фрагменты"))?;
    }

    let mut data = Vec::with_capacity(DATA_SHARDS * shard_size);
    for shard in shards.into_iter().take(DATA_SHARDS) {
        data.extend_from_slice(
            shard
                .as_ref()
                .ok_or_else(|| invalid("Фрагмент RS отсутствует"))?,
        );
    }

    if data.len() < 4 {
        return Err(invalid("Восстановленные данные RS слишком короткие"));
    }
    let original_len = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
    let available = data.len() - 4;
    if original_len > available {
        return Err(invalid("Некорректная длина восстановленных данных RS"));
    }
    let payload = data[4..4 + original_len].to_vec();

    Ok((payload, repaired))
}
