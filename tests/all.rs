//! Проверки алгоритмов, блочного потока и команд архиватора.

#![allow(dead_code, unused_imports)]

#[path = "../src/archive/mod.rs"]
mod archive;
#[path = "../src/block/mod.rs"]
mod block;
#[path = "../src/ecc.rs"]
mod ecc;
#[path = "../src/lz77/mod.rs"]
mod lz77;
#[path = "../src/parallel.rs"]
mod parallel;
#[path = "../src/rans/mod.rs"]
mod rans;
#[path = "../src/varint.rs"]
mod varint;

use archive::EncodingOptions;
use block::{ARCHIVE_HEADER_SIZE, BLOCK_HEADER_SIZE, BlockReader, BlockWriter};
use lz77::matcher::{Matcher, hash_pair};
use parallel::Executor;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Mutex};
const BLOCK_SIZE: usize = 64 * 1024;
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn random_bytes(size: usize) -> Vec<u8> {
    let mut state = 0x1234_5678_u32;
    (0..size)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect()
}

fn length_prefix_size(bytes: &[u8]) -> usize {
    let mut rest = bytes;
    crate::varint::read(&mut rest).unwrap();
    bytes.len() - rest.len()
}

// === LZ77 ===

fn lz_hash_matches_reference_for_all_short_ternary_inputs() {
    for size in 0..=8 {
        for mut value in 0..3_usize.pow(size) {
            let input: Vec<_> = (0..size)
                .map(|_| {
                    let byte = (value % 3) as u8;
                    value /= 3;
                    byte
                })
                .collect();
            let encoded = lz77::encode(&input).unwrap();
            assert_eq!(encoded, naive_lz(&input), "{input:?}");
            assert_eq!(lz77::decode(&encoded).unwrap(), input);
        }
    }
    println!("  alphabet=3 | max_len=8 | inputs=9841");
}

fn lz_hash_matches_reference_for_repeated_prefixes() {
    let input: Vec<u8> = (0..=255).flat_map(|byte| [0, byte, 255]).collect();
    let input = [input.repeat(3), vec![0, 42, 254, 1]].concat();
    let encoded = lz77::encode(&input).unwrap();
    assert_eq!(encoded, naive_lz(&input));
    assert_eq!(lz77::decode(&encoded).unwrap(), input);
    println!("  input_len=2308");
}

fn lz_paper_example_and_overlap() {
    let encoded = lz77::encode(b"ABABABA").unwrap();
    assert_eq!(encoded, [7, 0, 65, 0, 66, 4, 2, 65]);
    assert_eq!(lz77::decode(&encoded).unwrap(), b"ABABABA");
    assert_eq!(lz77::encode(b"AAAAAA").unwrap(), [6, 0, 65, 4, 1, 65]);
    println!("  ABABABA=07 00 41 00 42 04 02 41");
}

fn naive_lz(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::new();
    varint::write(&mut result, data.len() as u64).unwrap();
    let mut position = 0;
    while position < data.len() {
        let mut best = (0, 0);
        for distance in 1..=4096.min(position) {
            let mut length = 0;
            while length < 255.min(data.len() - position - 1)
                && data[position - distance + length] == data[position + length]
            {
                length += 1;
            }
            if length > best.1 {
                best = (distance, length);
            }
        }
        result.push(best.1 as u8);
        if best.1 != 0 {
            varint::write(&mut result, best.0 as u64).unwrap();
        }
        result.push(data[position + best.1]);
        position += best.1 + 1;
    }
    result
}

fn lz_search_matches_exhaustive_reference() {
    for input in [
        b"abXabYabZ".to_vec(),
        random_bytes(1024),
        random_bytes(1024).into_iter().map(|b| b % 4).collect(),
        vec![b'A'; 1024],
    ] {
        assert_eq!(lz77::encode(&input).unwrap(), naive_lz(&input));
    }
    println!("  inputs=4");
}

fn lz_match_and_window_boundaries() {
    let input = vec![b'A'; 258];
    let encoded = lz77::encode(&input).unwrap();
    assert_eq!(lz77::decode(&encoded).unwrap(), input);
    for gap in [4094, 4095] {
        let input = [b"AB".as_slice(), &vec![0; gap], b"AB!"].concat();
        let encoded = lz77::encode(&input).unwrap();
        assert_eq!(lz77::decode(&encoded).unwrap(), input);
    }
    println!("  run_len=258 | gaps=4094,4095");
}

// === rANS ===

fn rans_encode_decode_roundtrip() {
    assert_eq!(rans::encode(&[]).unwrap(), [0]);
    assert_eq!(rans::decode(&[0]).unwrap(), []);
    let encoded = rans::encode(b"ABAB").unwrap();
    assert_eq!(encoded, [4, 1, 65, 66, 0x80, 0x10, 0, 80, 0, 8]);
    assert_eq!(rans::decode(&encoded).unwrap(), b"ABAB");
    for distinct in [1, 2, 31, 32, 33, 127, 128, 255, 256] {
        let input: Vec<u8> = (0..distinct).map(|s| s as u8).collect();
        let encoded = rans::encode(&input).unwrap();
        assert_eq!(rans::decode(&encoded).unwrap(), input);
    }
    let input = random_bytes(4096);
    assert_eq!(rans::decode(&rans::encode(&input).unwrap()).unwrap(), input);
    println!("  distinct=1..256 | roundtrip=ok");
}

// === Reed-Solomon (erasure) ===

fn rs_encode_decode_roundtrip() {
    for size in [1, 10, 100, 1000, 65536] {
        let data = random_bytes(size);
        let encoded = ecc::encode(&data);
        assert_eq!(encoded.len(), ecc::encoded_size(size));
        let (decoded, repaired) = ecc::decode(&encoded, size).unwrap();
        assert_eq!(decoded, data);
        assert_eq!(repaired, 0);
    }
    println!("  sizes=1,10,100,1000,65536 | roundtrip=ok");
}

fn rs_corrects_damaged_shards() {
    let data = random_bytes(1000);
    let mut encoded = ecc::encode(&data);
    // Повреждаем 2 фрагмента (максимум для parity=2)
    let shard_size = (data.len() + 4 + 9) / 10; // div_ceil(data+4, 10)
    let crc_table_end = 12 * 4;
    // Портим первый фрагмент
    encoded[crc_table_end] ^= 0xFF;
    encoded[crc_table_end + 1] ^= 0xFF;
    // Портим второй фрагмент
    encoded[crc_table_end + shard_size] ^= 0xFF;
    let (decoded, repaired) = ecc::decode(&encoded, data.len()).unwrap();
    assert_eq!(decoded, data);
    assert_eq!(repaired, 2);
    println!("  damaged_shards=2 | repaired=2");
}

fn rs_too_many_damaged_shards_fails() {
    let data = random_bytes(1000);
    let mut encoded = ecc::encode(&data);
    let shard_size = (data.len() + 4 + 9) / 10;
    let crc_table_end = 12 * 4;
    // Портим 3 фрагмента (больше чем parity=2)
    for i in 0..3 {
        encoded[crc_table_end + i * shard_size] ^= 0xFF;
    }
    assert!(ecc::decode(&encoded, data.len()).is_err());
    println!("  damaged=3 | parity=2 | result=error");
}

// === Троирование ===

fn triplicate_detriplicate_roundtrip() {
    let data = b"CLARSv02".to_vec();
    let tripled = block::triplicate(&data);
    assert_eq!(tripled.len(), data.len() * 3);
    let restored = block::detriplicate(&tripled, data.len()).unwrap();
    assert_eq!(restored, data);
    println!("  roundtrip=ok");
}

fn triplicate_corrects_single_copy() {
    let data = vec![1, 2, 3, 4];
    let mut tripled = block::triplicate(&data);
    // Портим первую копию
    tripled[0] = 0xFF;
    tripled[1] = 0xFF;
    let restored = block::detriplicate(&tripled, data.len()).unwrap();
    assert_eq!(restored, data);
    // Портим вторую копию
    let mut tripled = block::triplicate(&data);
    tripled[4] = 0xFF;
    let restored = block::detriplicate(&tripled, data.len()).unwrap();
    assert_eq!(restored, data);
    println!("  single_copy_damaged=ok");
}

fn triplicate_fails_on_all_different() {
    let mut tripled = vec![1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3, 4];
    tripled[0] = 0xAA;
    tripled[4] = 0xBB;
    tripled[8] = 0xCC;
    assert!(block::detriplicate(&tripled, 4).is_none());
    println!("  all_different=None");
}

// === Блочный поток ===

fn block_stream(input: &[u8], protected: bool) -> Vec<u8> {
    let mut writer = BlockWriter::with_threads(
        Vec::new(),
        EncodingOptions { protected, ..Default::default() },
        None,
    ).unwrap();
    for part in input.chunks(137) {
        writer.write_all(part).unwrap();
    }
    writer.finish().unwrap()
}

fn block_roundtrip_with_and_without_protection() {
    for protected in [false, true] {
        for input in [vec![b'A'; BLOCK_SIZE + 1], random_bytes(BLOCK_SIZE + 123)] {
            let stream = block_stream(&input, protected);
            let mut reader = BlockReader::with_threads(stream.as_slice(), None).unwrap();
            let mut output = vec![0; input.len()];
            reader.read_exact(&mut output).unwrap();
            let stats = reader.finish().unwrap();
            assert_eq!(output, input);
            assert_eq!(stats.options.protected, protected);
            assert_eq!(stats.compressed + stats.stored, 2);
        }
    }
    println!("  protect=false,true | blocks=2 | restored=input");
}

fn all_eight_encoding_modes_roundtrip() {
    for flags in 0..8 {
        let options = EncodingOptions::from_flags(flags).unwrap();
        for input in [
            b"ABABABA".repeat(2000),
            vec![42; BLOCK_SIZE],
            random_bytes(BLOCK_SIZE),
            vec![7],
        ] {
            let mut writer = BlockWriter::with_threads(Vec::new(), options, Some(1)).unwrap();
            writer.write_all(&input).unwrap();
            let bytes = writer.finish().unwrap();
            let mut reader = BlockReader::with_threads(bytes.as_slice(), Some(1)).unwrap();
            let mut restored = vec![0; input.len()];
            reader.read_exact(&mut restored).unwrap();
            let stats = reader.finish().unwrap();
            assert_eq!(restored, input);
            assert_eq!(stats.options, options);
        }
    }
    println!("  flags=0..7 | inputs=4 | roundtrip=ok");
}

// === Многопоточность ===

fn two_workers_run_concurrently() {
    let mut executor = Executor::new(Some(2)).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let ids = Mutex::new(Vec::new());
    let result = executor
        .map(vec![10, 20], |value| {
            ids.lock().unwrap().push(std::thread::current().id());
            barrier.wait();
            value * 2
        })
        .unwrap();
    assert_eq!(result, [20, 40]);
    let ids = ids.into_inner().unwrap();
    assert_ne!(ids[0], ids[1]);
    println!("  workers=2 | results=20,40");
}

fn data(size: usize) -> Vec<u8> {
    let mut state = 0x12345678_u32;
    (0..size)
        .map(|i| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            if (i / BLOCK_SIZE).is_multiple_of(2) { b'A' } else { state as u8 }
        })
        .collect()
}

fn stream(input: &[u8], protected: bool, threads: usize) -> Vec<u8> {
    let mut writer = BlockWriter::with_threads(
        Vec::new(),
        EncodingOptions { protected, ..Default::default() },
        Some(threads),
    ).unwrap();
    for part in input.chunks(137) {
        writer.write_all(part).unwrap();
    }
    writer.finish().unwrap()
}

fn parallel_batches_identical_and_roundtrip() {
    for protected in [false, true] {
        for size in [0, 1, BLOCK_SIZE, BLOCK_SIZE + 1, 4 * BLOCK_SIZE + 17] {
            let input = data(size);
            let reference = stream(&input, protected, 1);
            for threads in [2, 3] {
                let packed = stream(&input, protected, threads);
                assert_eq!(packed, reference);
                let mut reader = BlockReader::with_threads(packed.as_slice(), Some(threads)).unwrap();
                let mut restored = vec![0; size];
                reader.read_exact(&mut restored).unwrap();
                assert_eq!(restored, input);
                reader.finish().unwrap();
            }
        }
    }
    println!("  protected=false,true | threads=2,3 | roundtrip=ok");
}

// === Команды архиватора ===

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Sandbox(PathBuf);
impl Sandbox {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!(
            "archiver-test-{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf { self.0.join(name) }
    fn command(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_arch"))
            .current_dir(&self.0).args(args).output().unwrap()
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) { fs::remove_dir_all(&self.0).expect("cleanup"); }
}

fn compare_tree(left: &Path, right: &Path) {
    let names = |path: &Path| {
        let mut names: Vec<_> = fs::read_dir(path).unwrap()
            .map(|item| item.unwrap().file_name()).collect();
        names.sort();
        names
    };
    assert_eq!(names(left), names(right));
    for name in names(left) {
        let a = left.join(&name);
        let b = right.join(&name);
        if a.is_dir() {
            assert!(b.is_dir());
            compare_tree(&a, &b);
        } else {
            assert_eq!(fs::read(a).unwrap(), fs::read(b).unwrap());
        }
    }
}

fn cli_roundtrip() {
    for protect in [false, true] {
        let sandbox = Sandbox::new();
        fs::create_dir_all(sandbox.path("папка/empty-dir")).unwrap();
        fs::write(sandbox.path("папка/empty-file"), []).unwrap();
        fs::write(sandbox.path("папка/данные.txt"), "Пример\n".repeat(20_000)).unwrap();
        fs::write(sandbox.path("binary"), (0..=255).cycle().take(70_000).collect::<Vec<u8>>()).unwrap();
        let mut args = vec!["create"];
        if protect { args.push("--protect"); }
        args.extend(["result", "папка", "binary"]);
        let output = sandbox.command(&args);
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(sandbox.path("result.mrx").is_file());
        let info = sandbox.command(&["info", "result"]);
        assert!(info.status.success());
        let output = sandbox.command(&["extract", "result", "out"]);
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        compare_tree(&sandbox.path("папка"), &sandbox.path("out/папка"));
        assert_eq!(fs::read(sandbox.path("binary")).unwrap(), fs::read(sandbox.path("out/binary")).unwrap());
    }
    println!("  protect=false,true | roundtrip=ok");
}

fn all_eight_modes_cli() {
    let sandbox = Sandbox::new();
    fs::create_dir_all(sandbox.path("input/пустая")).unwrap();
    fs::write(sandbox.path("input/empty"), []).unwrap();
    fs::write(sandbox.path("input/text"), b"ABABABA".repeat(12000)).unwrap();
    fs::write(sandbox.path("input/bytes"), (0..=255).cycle().take(70000).collect::<Vec<u8>>()).unwrap();
    for flags in 0..8 {
        let name = format!("mode-{flags}.mrx");
        let mut args = vec!["create", "--threads", "2"];
        if flags & 1 == 0 { args.push("--no-lz77"); }
        if flags & 2 == 0 { args.push("--no-rans"); }
        if flags & 4 != 0 { args.push("--protect"); }
        args.extend([name.as_str(), "input"]);
        let result = sandbox.command(&args);
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        let out = format!("out-{flags}");
        assert!(sandbox.command(&["extract", &name, &out]).status.success());
        compare_tree(&sandbox.path("input"), &sandbox.path(&format!("{out}/input")));
    }
    println!("  flags=0..7 | roundtrip=ok");
}

fn main() {
    let tests: &[(&str, &str, fn())] = &[
        ("LZ77", "Перебор коротких входов", lz_hash_matches_reference_for_all_short_ternary_inputs),
        ("LZ77", "Повторяющиеся префиксы", lz_hash_matches_reference_for_repeated_prefixes),
        ("LZ77", "Пример ABABABA", lz_paper_example_and_overlap),
        ("LZ77", "Сравнение с прямым поиском", lz_search_matches_exhaustive_reference),
        ("LZ77", "Границы длины и окна", lz_match_and_window_boundaries),
        ("rANS", "Кодирование и декодирование", rans_encode_decode_roundtrip),
        ("Reed-Solomon", "Кодирование и декодирование", rs_encode_decode_roundtrip),
        ("Reed-Solomon", "Исправление повреждённых фрагментов", rs_corrects_damaged_shards),
        ("Reed-Solomon", "Превышение числа повреждений", rs_too_many_damaged_shards_fails),
        ("Троирование", "Кодирование и декодирование", triplicate_detriplicate_roundtrip),
        ("Троирование", "Исправление одной копии", triplicate_corrects_single_copy),
        ("Троирование", "Все копии различны", triplicate_fails_on_all_different),
        ("Блочный поток", "Запись и чтение с защитой", block_roundtrip_with_and_without_protection),
        ("Блочный поток", "Все восемь режимов", all_eight_encoding_modes_roundtrip),
        ("Многопоточность", "Параллельное выполнение", two_workers_run_concurrently),
        ("Многопоточность", "Партии и границы", parallel_batches_identical_and_roundtrip),
        ("Команды", "Создание и распаковка", cli_roundtrip),
        ("Команды", "Восемь режимов через CLI", all_eight_modes_cli),
    ];
    let mut previous_group = "";
    let mut failures = 0;
    for &(group, name, test) in tests {
        if group != previous_group {
            println!("\n{group}");
            previous_group = group;
        }
        println!("{name}");
        if std::panic::catch_unwind(test).is_ok() {
            println!("  OK");
        } else {
            println!("  ERR");
            failures += 1;
        }
    }
    println!("\nOK: {}\nERR: {failures}", tests.len() - failures);
    if failures != 0 { std::process::exit(1); }
}
