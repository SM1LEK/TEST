use super::{
    Entry, Statistics, cleanup_error,
    paths::{MAX_PATH_BYTES, Names, archive_path},
};
use crate::{block::BlockReader, invalid, varint};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read};
use std::path::Path;

fn read_name(input: &mut impl Read) -> io::Result<String> {
    let length = varint::read_bounded(input, MAX_PATH_BYTES)?;
    if length == 0 {
        return Err(invalid("Пустой путь"));
    }
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes)?;
    String::from_utf8(bytes).map_err(|_| invalid("Путь не UTF8"))
}

fn process(
    path: &Path,
    destination: Option<&Path>,
    threads: Option<usize>,
    mut on_entry: impl FnMut(&Entry),
) -> io::Result<Statistics> {
    let file = File::open(archive_path(path)?)?;
    let mut stats = Statistics {
        archive_bytes: file.metadata()?.len(),
        ..Statistics::default()
    };
    let mut reader = BlockReader::with_threads(BufReader::new(file), threads)?;
    let mut names = Names::default();
    loop {
        let mut kind = [0];
        reader.read_exact(&mut kind)?;
        if kind[0] == 0 {
            break;
        }
        if !matches!(kind[0], 1 | 2) {
            return Err(invalid("Неизвестный тип записи"));
        }
        let name = read_name(&mut reader)?;
        let directory = kind[0] == 2;
        names.add(&name, directory)?;

        let target = destination.map(|root| root.join(&name));
        let mut size = 0;

        if directory {
            stats.directories += 1;
            if let Some(target) = &target {
                fs::create_dir(target)?;
            }
        } else {
            stats.files += 1;
            size = varint::read(&mut reader)?;
            stats.original_bytes = stats
                .original_bytes
                .checked_add(size)
                .ok_or_else(|| invalid("Превышение лимита данных"))?;
            let mut output = target
                .as_ref()
                .map(|target| OpenOptions::new().write(true).create_new(true).open(target))
                .transpose()?;
            let mut source = (&mut reader).take(size);
            let copied = if let Some(sink) = output.as_mut() {
                io::copy(&mut source, sink)?
            } else {
                io::copy(&mut source, &mut io::sink())?
            };
            if copied != size {
                return Err(invalid("Неожиданный конец архива"));
            }
            if let Some(output) = output {
                output.sync_all()?;
            }
        }
        on_entry(&Entry {
            path: name,
            is_directory: directory,
            size,
        });
    }
    if names.entries.is_empty() {
        return Err(invalid("Пустой архив"));
    }
    let blocks = reader.finish()?;
    stats.protected = blocks.options.protected;
    stats.lz77 = blocks.options.lz77;
    stats.rans = blocks.options.rans;
    stats.compressed_blocks = blocks.compressed;
    stats.stored_blocks = blocks.stored;
    stats.corrected_shards = blocks.corrected_shards;
    Ok(stats)
}

pub fn inspect_with_threads(
    path: &Path,
    threads: Option<usize>,
    on_entry: impl FnMut(&Entry),
) -> io::Result<Statistics> {
    process(path, None, threads, on_entry)
}

pub fn extract_with_threads(
    path: &Path,
    destination: &Path,
    threads: Option<usize>,
) -> io::Result<Statistics> {
    fs::create_dir(destination)?;
    match process(path, Some(destination), threads, |_| {}) {
        Ok(stats) => Ok(stats),
        Err(error) => Err(cleanup_error(destination, true, error)),
    }
}
