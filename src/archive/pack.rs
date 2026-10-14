use super::{
    EncodingOptions, cleanup_error,
    paths::{Names, archive_path, file_name, is_link},
};
use crate::{block::BlockWriter, invalid, varint};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

fn write_name(output: &mut impl Write, name: &str) -> io::Result<()> {
    varint::write(output, name.len() as u64)?;
    output.write_all(name.as_bytes())
}

fn pack_file(output: &mut impl Write, path: &Path) -> io::Result<()> {
    let mut input = File::open(path)?;
    let size = input.metadata()?.len();
    varint::write(output, size)?;
    let copied = io::copy(&mut (&mut input).take(size), output)?;
    if copied != size || input.read(&mut [0u8; 1])? != 0 {
        return Err(invalid(&format!(
            "Данные были изменены: {}",
            path.display()
        )));
    }
    Ok(())
}

fn pack_path(
    output: &mut impl Write,
    path: &Path,
    name: &str,
    destination: &Path,
    names: &mut Names,
) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if is_link(&metadata) {
        return Err(invalid(&format!("Не путь: {}", path.display())));
    }

    // Исключаем создаваемый архив из упаковываемого каталога.
    if path.file_name() == destination.file_name() && fs::canonicalize(path)? == destination {
        return Ok(());
    }
    let directory = metadata.is_dir();
    if !directory && !metadata.is_file() {
        return Err(invalid(&format!("Нетипичный файл: {}", path.display())));
    }
    names.add(name, directory)?;
    output.write_all(&[if directory { 2 } else { 1 }])?;
    write_name(output, name)?;
    if directory {
        let mut children: Vec<_> = fs::read_dir(path)?.collect::<io::Result<_>>()?;
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            let child_path = child.path();
            let child_name = format!("{name}/{}", file_name(&child_path)?);
            pack_path(output, &child_path, &child_name, destination, names)?;
        }
    } else {
        pack_file(output, path)?;
    }
    Ok(())
}

pub fn create_with_options(
    path: &Path,
    sources: &[PathBuf],
    options: EncodingOptions,
    threads: Option<usize>,
) -> io::Result<PathBuf> {
    let destination = archive_path(path)?;
    if sources.is_empty() {
        return Err(invalid("Нет данных"));
    }
    let sources: Vec<_> = sources
        .iter()
        .map(std::path::absolute)
        .collect::<io::Result<_>>()?;

    for source in &sources {
        file_name(source)?;
        fs::symlink_metadata(source)?;
    }

    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)?;
    let result = (|| {
        let absolute_destination = fs::canonicalize(&destination)?;
        let mut writer = BlockWriter::with_threads(BufWriter::new(file), options, threads)?;
        let mut names = Names::default();
        for source in &sources {
            pack_path(
                &mut writer,
                source,
                file_name(source)?,
                &absolute_destination,
                &mut names,
            )?;
        }
        if names.entries.is_empty() {
            return Err(invalid("Нет данных"));
        }
        writer.write_all(&[0])?;
        let output = writer.finish()?;
        output
            .into_inner()
            .map_err(|error| error.into_error())?
            .sync_all()
    })();
    if let Err(error) = result {
        return Err(cleanup_error(&destination, false, error));
    }
    Ok(destination)
}
