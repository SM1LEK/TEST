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
use block::{BlockReader, BlockWriter};
use std::fs;
use std::hint::black_box;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const BLOCK_SIZE: usize = 64 * 1024;
const MIB: usize = 1024 * 1024;
const WARMUP: usize = 1;
static NEXT: AtomicU64 = AtomicU64::new(0);

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn setting(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .map(|value| value.parse().expect("Не целое число"))
        .filter(|&value| value != 0)
        .unwrap_or(default)
}

fn input_size() -> usize {
    setting("CLARSLIB_BENCH_BYTES", MIB)
}

fn runs() -> usize {
    setting("CLARSLIB_BENCH_RUNS", 5)
}

fn random_data(size: usize) -> Vec<u8> {
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

fn repeated_data(size: usize) -> Vec<u8> {
    b"one two three four\n"
        .iter()
        .copied()
        .cycle()
        .take(size)
        .collect()
}

fn mixed_data(size: usize) -> Vec<u8> {
    let mut data = random_data(size);
    for (number, block) in data.chunks_mut(BLOCK_SIZE).enumerate() {
        if number.is_multiple_of(2) {
            block.fill(b'A');
        }
    }
    data
}

fn inputs() -> Vec<(&'static str, Vec<u8>)> {
    let size = input_size();
    vec![
        ("Повторяющийся текст", repeated_data(size)),
        ("Псевдослучайные данные", random_data(size)),
        ("Смешанные данные", mixed_data(size)),
    ]
}

fn pack(input: &[u8], options: EncodingOptions, threads: usize) -> Vec<u8> {
    let mut writer = BlockWriter::with_threads(Vec::new(), options, Some(threads)).unwrap();
    writer.write_all(black_box(input)).unwrap();
    writer.finish().unwrap()
}

fn unpack(input: &[u8], size: usize, threads: usize) -> Vec<u8> {
    let mut reader = BlockReader::with_threads(black_box(input), Some(threads)).unwrap();
    let mut output = vec![0; size];
    reader.read_exact(&mut output).unwrap();
    reader.finish().unwrap();
    output
}

fn median(times: &mut [Duration]) -> Duration {
    times.sort_unstable();
    let middle = times.len() / 2;
    if times.len().is_multiple_of(2) {
        times[middle - 1] / 2 + times[middle] / 2
    } else {
        times[middle]
    }
}

fn measure(mut operation: impl FnMut()) -> Duration {
    for _ in 0..WARMUP {
        operation();
    }
    let mut times = Vec::with_capacity(runs());
    for _ in 0..runs() {
        let start = Instant::now();
        operation();
        times.push(start.elapsed());
    }
    median(&mut times)
}

fn speed(bytes: usize, time: Duration) -> f64 {
    bytes as f64 / MIB as f64 / time.as_secs_f64().max(f64::MIN_POSITIVE)
}

fn print_time(name: &str, bytes: usize, time: Duration) {
    println!(
        "  {name}: {:.3} мс, {:.3} МиБ/с",
        time.as_secs_f64() * 1000.0,
        speed(bytes, time)
    );
}

fn print_size(original: usize, packed: usize) {
    let ratio = packed as f64 / original as f64;
    println!(
        "  Размер: {original} -> {packed} байт, экономия {:.3}%",
        (1.0 - ratio) * 100.0
    );
}

fn overall() {
    for (name, input) in inputs() {
        let packed = pack(&input, EncodingOptions::default(), 1);
        assert_eq!(unpack(&packed, input.len(), 1), input);
        let pack_time = measure(|| {
            black_box(pack(&input, EncodingOptions::default(), 1));
        });
        let unpack_time = measure(|| {
            black_box(unpack(&packed, input.len(), 1));
        });
        println!("  \nНабор: {name}");
        print_size(input.len(), packed.len());
        print_time("Упаковка", input.len(), pack_time);
        print_time("Распаковка", input.len(), unpack_time);
    }
}

fn encode_parts(input: &[u8], options: EncodingOptions) -> Vec<Vec<u8>> {
    input
        .chunks(BLOCK_SIZE)
        .map(|block| {
            let mut data = if options.lz77 {
                lz77::encode(black_box(block)).unwrap()
            } else {
                block.to_vec()
            };
            if options.rans {
                data = rans::encode(&data).unwrap();
            }
            data
        })
        .collect()
}

fn decode_parts(parts: &[Vec<u8>], options: EncodingOptions) -> Vec<u8> {
    let mut output = Vec::new();
    for part in parts {
        let mut data = if options.rans {
            rans::decode(black_box(part)).unwrap()
        } else {
            part.clone()
        };
        if options.lz77 {
            data = lz77::decode(&data).unwrap();
        }
        output.extend_from_slice(&data);
    }
    output
}

fn stages() {
    let cases = [
        (
            "LZ77",
            EncodingOptions {
                lz77: true,
                rans: false,
                protected: false,
            },
        ),
        (
            "rANS",
            EncodingOptions {
                lz77: false,
                rans: true,
                protected: false,
            },
        ),
        ("LZ77 + rANS", EncodingOptions::default()),
    ];
    for (input_name, input) in inputs() {
        println!("  \nНабор: {input_name}");
        for (name, options) in cases {
            let parts = encode_parts(&input, options);
            assert_eq!(decode_parts(&parts, options), input);
            let encode_time = measure(|| {
                black_box(encode_parts(&input, options));
            });
            let decode_time = measure(|| {
                black_box(decode_parts(&parts, options));
            });
            println!("  Этап: {name}");
            print_time("Кодирование", input.len(), encode_time);
            print_time("Декодирование", input.len(), decode_time);
        }
    }
}

fn threads() {
    let input = mixed_data(input_size().max(BLOCK_SIZE * 32));
    let reference = pack(&input, EncodingOptions::default(), 1);
    let baseline = measure(|| {
        black_box(pack(&input, EncodingOptions::default(), 1));
    });
    for count in [1, 2, 4] {
        let packed = pack(&input, EncodingOptions::default(), count);
        assert_eq!(packed, reference);
        assert_eq!(unpack(&packed, input.len(), count), input);
        let time = if count == 1 {
            baseline
        } else {
            measure(|| {
                black_box(pack(&input, EncodingOptions::default(), count));
            })
        };
        println!(
            "  \nПотоков: {count}, ускорение: {:.3}",
            baseline.as_secs_f64() / time.as_secs_f64().max(f64::MIN_POSITIVE)
        );
        print_time("Упаковка", input.len(), time);
    }
}

fn rs_cost() {
    let input = mixed_data(input_size());
    let mut baseline: Option<(usize, Duration, Duration)> = None;
    for protected in [false, true] {
        let options = EncodingOptions {
            protected,
            ..Default::default()
        };
        let packed = pack(&input, options, 1);
        assert_eq!(unpack(&packed, input.len(), 1), input);
        let pack_time = measure(|| {
            black_box(pack(&input, options, 1));
        });
        let unpack_time = measure(|| {
            black_box(unpack(&packed, input.len(), 1));
        });
        println!(
            "  \nRS: {}",
            if protected {
                "включён"
            } else {
                "выключен"
            }
        );
        print_size(input.len(), packed.len());
        print_time("Упаковка", input.len(), pack_time);
        print_time("Распаковка", input.len(), unpack_time);
        if let Some((base_size, base_pack, base_unpack)) = baseline {
            println!(
                "  \nРазмер +{:.3}%, упаковка x{:.3}, распаковка x{:.3}",
                (packed.len() as f64 / base_size as f64 - 1.0) * 100.0,
                pack_time.as_secs_f64() / base_pack.as_secs_f64(),
                unpack_time.as_secs_f64() / base_unpack.as_secs_f64()
            );
        } else {
            baseline = Some((packed.len(), pack_time, unpack_time));
        }
    }
}

struct Sandbox(PathBuf);

impl Sandbox {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "archiver-bench-{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn same_tree(left: &Path, right: &Path) {
    let names = |path: &Path| {
        let mut names: Vec<_> = fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        names
    };
    let left_names = names(left);
    assert_eq!(left_names, names(right));
    for name in left_names {
        let left = left.join(&name);
        let right = right.join(&name);
        if left.is_dir() {
            assert!(right.is_dir());
            same_tree(&left, &right);
        } else {
            assert_eq!(fs::read(left).unwrap(), fs::read(right).unwrap());
        }
    }
}

fn files_size(path: &Path) -> u64 {
    let metadata = fs::symlink_metadata(path).unwrap();
    assert!(!metadata.file_type().is_symlink());
    if metadata.is_dir() {
        fs::read_dir(path)
            .unwrap()
            .map(|entry| files_size(&entry.unwrap().path()))
            .sum()
    } else {
        metadata.len()
    }
}

fn archive() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("demo_corpus");
    assert!(corpus.is_dir(), "Не найден demo_corpus");
    let original_size = usize::try_from(files_size(&corpus)).unwrap();
    let sandbox = Sandbox::new();
    let destination = sandbox.path("result.mrx");
    let output = sandbox.path("out");
    let start = Instant::now();
    let packed = archive::create_with_options(
        &destination,
        std::slice::from_ref(&corpus),
        EncodingOptions::default(),
        Some(1),
    )
    .unwrap();
    let pack_time = start.elapsed();
    let archive_size = usize::try_from(fs::metadata(&packed).unwrap().len()).unwrap();
    let start = Instant::now();
    archive::extract_with_threads(&packed, &output, Some(1)).unwrap();
    let unpack_time = start.elapsed();
    same_tree(&corpus, &output.join("demo_corpus"));
    print_size(original_size, archive_size);
    print_time("Создание архива", original_size, pack_time);
    print_time("Извлечение архива", original_size, unpack_time);
}

fn main() {
    assert!(std::env::args_os().nth(1).is_none(), "Аргументы не нужны");
    println!("Объем данных: {}, измерений: {}", input_size(), runs());

    let benches: &[(&str, fn())] = &[
        ("\nОбщая скорость и размер", overall),
        ("\nСкорость LZ77 и rANS", stages),
        ("\nМногопоточность", threads),
        ("\nReed-Solomon", rs_cost),
        ("\ndemo_corpus", archive),
    ];
    for &(name, bench) in benches {
        println!("\n{name}");
        bench();
    }
    println!("\nOK: {}", benches.len());
}
