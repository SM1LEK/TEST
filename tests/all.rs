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

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
const BLOCK_SIZE: usize = 64 * 1024;
const RS_DATA_SHARDS: usize = 10;
const RS_TOTAL_SHARDS: usize = 12;
const RS_LENGTH_SIZE: usize = 4;
const CRC_SIZE: usize = 4;
const RS_CRC_TABLE_SIZE: usize = RS_TOTAL_SHARDS * CRC_SIZE;
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
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

fn lz_simple() {
    let input = b"ABABABA";
    let encoded = lz77::encode(input).unwrap();
    assert_eq!(encoded, [7, 0, 65, 0, 66, 4, 2, 65]);
    let decoded = lz77::decode(&encoded).unwrap();
    assert_eq!(decoded, input);
    println!("  IN: ABABABA");
    println!("  OUT: 07 00 41 00 42 04 02 41");
}

fn lz_complex() {
    let input = "one два 3 0x4 one два 3 0x4 one два 3 0x4";
    let encoded = lz77::encode(input.as_bytes()).unwrap();
    let decoded = lz77::decode(&encoded).unwrap();
    assert_eq!(decoded, input.as_bytes());
    println!("  IN: {input}");
    println!("  OUT: {} байт после LZ77", encoded.len());
}

fn lz_limits() {
    for input in [Vec::new(), vec![b'A'; 256], random_data(4097)] {
        let encoded = lz77::encode(&input).unwrap();
        assert_eq!(lz77::decode(&encoded).unwrap(), input);
    }
    println!("  IN: 0, 256, 4097 байт");
}

fn rans_simple() {
    let input = "БАНАН";
    let encoded = rans::encode(input.as_bytes()).unwrap();
    let decoded = rans::decode(&encoded).unwrap();
    assert_eq!(decoded, input.as_bytes());
    println!("  IN: {input}");
    println!("  OUT: {} байт после rANS", encoded.len());
}

fn rans_complex() {
    let input = random_data(4096);
    let encoded = rans::encode(&input).unwrap();
    assert_eq!(rans::decode(&encoded).unwrap(), input);
    println!("  IN: 4096 псевдослучайных байт");
    println!("  OUT: {} байт после rANS", encoded.len());
}

fn rans_limits() {
    for input in [Vec::new(), (0..=255).collect()] {
        let encoded = rans::encode(&input).unwrap();
        assert_eq!(rans::decode(&encoded).unwrap(), input);
    }
    println!("  IN: пустой набор и все 256 значений байта");
}

fn damage_rs(encoded: &mut [u8], data_size: usize, shard: usize) {
    let shard_size = (data_size + RS_LENGTH_SIZE).div_ceil(RS_DATA_SHARDS);
    encoded[RS_CRC_TABLE_SIZE + shard * shard_size] ^= 0xFF;
}

fn rs_one() {
    let data = random_data(1000);
    let mut encoded = ecc::encode(&data);
    damage_rs(&mut encoded, data.len(), 0);
    let (decoded, repaired) = ecc::decode(&encoded).unwrap();
    assert_eq!(decoded, data);
    assert_eq!(repaired, 1);
    println!("  IN: 1000 байт, повреждён 1 фрагмент");
    println!("  Исправлен 1 фрагмент");
}

fn rs_two() {
    let data = random_data(1000);
    let mut encoded = ecc::encode(&data);
    damage_rs(&mut encoded, data.len(), 0);
    damage_rs(&mut encoded, data.len(), 1);
    let (decoded, repaired) = ecc::decode(&encoded).unwrap();
    assert_eq!(decoded, data);
    assert_eq!(repaired, 2);
    println!("  IN: 1000 байт, повреждены 2 фрагмента");
    println!("  Исправлены 2 фрагмента");
}

fn rs_three() {
    let data = random_data(1000);
    let mut encoded = ecc::encode(&data);
    for i in 0..3 {
        damage_rs(&mut encoded, data.len(), i);
    }
    assert!(ecc::decode(&encoded).is_err());
    println!("  IN: 1000 байт, повреждены 3 фрагмента");
    println!("  Восстановление отклонено");
}

fn triple_one() {
    let data = vec![1, 2, 3, 4];
    let mut tripled = block::triplicate(&data);
    tripled[0] = 0xFF;
    tripled[1] = 0xFF;
    let restored = block::detriplicate(&tripled, data.len()).unwrap();
    assert_eq!(restored, data);
    println!("  IN: 4 байта, повреждена 1 копия");
    println!("  Восстановлено по двум копиям");
}

fn triple_two() {
    let data = vec![1, 2, 3, 4];
    let mut tripled = block::triplicate(&data);
    tripled[0] = 0xAA;
    tripled[data.len() + 1] = 0xBB;
    let restored = block::detriplicate(&tripled, data.len()).unwrap();
    assert_eq!(restored, data);
    println!("  IN: 4 байта, повреждены разные места 2 копий");
    println!("  Восстановлено по двум копиям");
}

fn triple_three() {
    let data = vec![1, 2, 3, 4];
    let mut tripled = block::triplicate(&data);
    tripled[0] = 0xAA;
    tripled[data.len()] = 0xBB;
    tripled[data.len() * 2] = 0xCC;
    assert!(block::detriplicate(&tripled, data.len()).is_none());
    println!("  IN: 4 байта, один байт разный во всех 3 копиях");
    println!("  Значение выбрать невозможно");
}

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Sandbox(PathBuf);
impl Sandbox {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "architester-{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn command(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_arch"))
            .current_dir(&self.0)
            .args(args)
            .output()
            .unwrap()
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
            .map(|item| item.unwrap().file_name())
            .collect();
        names.sort();
        names
    };
    let left_names = names(left);
    assert_eq!(left_names, names(right));
    for name in left_names {
        let a = left.join(&name);
        let b = right.join(&name);
        if a.is_dir() {
            assert!(b.is_dir());
            same_tree(&a, &b);
        } else {
            assert_eq!(fs::read(a).unwrap(), fs::read(b).unwrap());
        }
    }
}

fn archive_test() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("demo_corpus");
    assert!(corpus.is_dir(), "Не найден тестовый корпус");

    for protect in [false, true] {
        let sandbox = Sandbox::new();
        let mut args = vec!["create"];
        if protect {
            args.push("--protect");
        }
        args.extend(["result", corpus.to_str().unwrap()]);
        let output = sandbox.command(&args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(sandbox.path("result.mrx").is_file());
        let info = sandbox.command(&["info", "result"]);
        let info_text = String::from_utf8_lossy(&info.stdout);
        assert!(info.status.success(), "{info_text}");
        assert!(info_text.contains("Файлов:"));
        assert!(info_text.contains("Защита:"));
        let output = sandbox.command(&["extract", "result", "out"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        same_tree(&corpus, &sandbox.path("out/demo_corpus"));
    }
    println!("  demo_corpus, без защиты и с RS");
}

fn main() {
    let tests: &[(&str, fn())] = &[
        ("LZ77", lz_simple),
        ("LZ77", lz_complex),
        ("LZ77", lz_limits),
        ("rANS", rans_simple),
        ("rANS", rans_complex),
        ("rANS", rans_limits),
        ("Reed-Solomon", rs_one),
        ("Reed-Solomon", rs_two),
        ("Reed-Solomon", rs_three),
        ("Троирование", triple_one),
        ("Троирование", triple_two),
        ("Троирование", triple_three),
        ("Архиватор", archive_test),
    ];
    let mut previous_group = "";
    let mut failures = 0;
    for &(group, test) in tests {
        if group != previous_group {
            println!("\n{group}");
            previous_group = group;
        }
        if std::panic::catch_unwind(test).is_ok() {
            println!("  OK\n");
        } else {
            println!("  ERR\n");
            failures += 1;
        }
    }
    println!("\nOK: {}\nERR: {failures}", tests.len() - failures);
    if failures != 0 {
        std::process::exit(1);
    }
}
