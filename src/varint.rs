use std::io::{self, Read, Write};

use crate::invalid;

pub(crate) fn write(output: &mut impl Write, mut value: u64) -> io::Result<()> {
    loop {
        let more = value >= 128;
        output.write_all(&[(value as u8 & 0x7F) | ((more as u8) << 7)])?;
        value >>= 7;
        if !more {
            return Ok(());
        }
    }
}

pub(crate) fn read(input: &mut impl Read) -> io::Result<u64> {
    let mut value = 0_u64;
    let mut byte = [0u8];
    for index in 0..10 {
        input.read_exact(&mut byte)?;
        let current = byte[0];
        // В десятом байте u64 допустим только один значащий бит.
        if index == 9 && current > 1 {
            return Err(invalid("Переполнение varint"));
        }
        value |= u64::from(current & 0x7F) << (index * 7);
        if current & 0x80 == 0 {
            if index != 0 && current == 0 {
                return Err(invalid("Неканоническая запись varint"));
            }
            return Ok(value);
        }
    }
    Err(invalid("Слишком длинный varint"))
}

pub(crate) fn read_bounded(input: &mut impl Read, maximum: usize) -> io::Result<usize> {
    let value = read(input)?;
    if value > maximum as u64 {
        return Err(invalid("переполнение значения varint"));
    }
    Ok(value as usize)
}
