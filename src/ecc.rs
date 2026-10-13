use std::io;

use crc32fast::Hasher as Crc32;
use reed_solomon_erasure::galois_8::ReedSolomon;

use crate::invalid;

const DATA_SHARDS: usize = 10;
const PARITY_SHARDS: usize = 2;
const TOTAL_SHARDS: usize = DATA_SHARDS + PARITY_SHARDS;

pub(crate) fn encoded_size(size: usize) -> usize {
    let protected_len = size + 4;
    let shard_size = protected_len.div_ceil(DATA_SHARDS).max(1);
    TOTAL_SHARDS * 4 + TOTAL_SHARDS * shard_size
}

pub(crate) fn encode(data: &[u8]) -> Vec<u8> {
    let original_len = data.len() as u32;
    let protected_len = data.len() + 4;
    let shard_size = protected_len.div_ceil(DATA_SHARDS).max(1);

    let mut protected_data = Vec::with_capacity(protected_len);
    protected_data.extend_from_slice(&original_len.to_le_bytes());
    protected_data.extend_from_slice(data);

    let mut shards = vec![vec![0u8; shard_size]; TOTAL_SHARDS];
    for (index, chunk) in protected_data.chunks(shard_size).enumerate() {
        if index >= DATA_SHARDS { break; }
        shards[index][..chunk.len()].copy_from_slice(chunk);
    }

    let codec = ReedSolomon::new(DATA_SHARDS, PARITY_SHARDS).unwrap();
    codec.encode(&mut shards).unwrap();

    let mut output = Vec::with_capacity(TOTAL_SHARDS * 4 + TOTAL_SHARDS * shard_size);
    for shard in &shards {
        output.extend_from_slice(&crc32(shard).to_le_bytes());
    }
    for shard in &shards {
        output.extend_from_slice(shard);
    }
    output
}

pub(crate) fn decode(input: &[u8], _original_size: usize) -> io::Result<(Vec<u8>, u64)> {
    let crc_table_len = TOTAL_SHARDS * 4;
    if input.len() <= crc_table_len {
        return Err(invalid("Обрезанные данные RS"));
    }

    let body_len = input.len() - crc_table_len;
    if body_len % TOTAL_SHARDS != 0 {
        return Err(invalid("Некорректный размер данных RS"));
    }
    let shard_size = body_len / TOTAL_SHARDS;
    if shard_size == 0 {
        return Err(invalid("Нулевой размер фрагмента RS"));
    }

    let mut expected_crc = Vec::with_capacity(TOTAL_SHARDS);
    for i in 0..TOTAL_SHARDS {
        let pos = i * 4;
        expected_crc.push(u32::from_le_bytes(input[pos..pos + 4].try_into().unwrap()));
    }

    let body = &input[crc_table_len..];
    let mut shards: Vec<Option<Vec<u8>>> = Vec::with_capacity(TOTAL_SHARDS);
    let mut repaired = 0u64;
    for i in 0..TOTAL_SHARDS {
        let start = i * shard_size;
        let end = start + shard_size;
        let shard = body[start..end].to_vec();
        if crc32(&shard) == expected_crc[i] {
            shards.push(Some(shard));
        } else {
            shards.push(None);
            repaired += 1;
        }
    }

    if repaired > PARITY_SHARDS as u64 {
        return Err(invalid("Слишком много повреждённых фрагментов"));
    }

    if repaired > 0 {
        let codec = ReedSolomon::new(DATA_SHARDS, PARITY_SHARDS)
            .map_err(|_| invalid("Ошибка создания кодека RS"))?;
        codec
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

fn crc32(bytes: &[u8]) -> u32 {
    let mut hasher = Crc32::new();
    hasher.update(bytes);
    hasher.finalize()
}
