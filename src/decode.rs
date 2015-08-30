/* Copyright 2015 Jordan Miner
 *
 * Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
 * http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
 * <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
 * option. This file may not be copied, modified, or distributed
 * except according to those terms.
 */

use std::borrow::Cow;
use std::io::{self, Read};
use std::iter::{repeat, Repeat};
use std::mem;
use std::string::FromUtf8Error;
use std::str::Utf8Error;
use super::{Value, Schema};

#[derive(Debug)]
pub enum DecodeErrorKind {
    ReadFailed(io::Error),
    UnexpectedEof,
    InvalidBoolean,
    IntegerOverflow,
    InvalidUtf8(Utf8Error),
}

#[derive(Debug)]
pub struct DecodeError {
    pub kind: DecodeErrorKind,
}

impl From<io::Error> for DecodeError {
    fn from(err: io::Error) -> DecodeError {
        DecodeError { kind: DecodeErrorKind::ReadFailed(err) }
    }
}

impl From<FromUtf8Error> for DecodeError {
    fn from(err: FromUtf8Error) -> DecodeError {
        DecodeError { kind: DecodeErrorKind::InvalidUtf8(err.utf8_error()) }
    }
}

// TODO: delete and switch over to Read::read_exact() when it is marked stable in Rust
fn read_exact<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<(), DecodeError> {
    let mut index = 0;
    let len = buf.len(); // borrow checker workaround
    while index < len {
        let read_count = try!(reader.read(&mut buf[index..len]));
        index += read_count;
        if read_count == 0 {
            return Err(DecodeError { kind: DecodeErrorKind::UnexpectedEof });
        }
    };
    Ok(())
}

fn decode_var_len_u64<R: Read>(reader: &mut R) -> Result<u64, DecodeError> {
    let mut num = 0;
    let mut i = 0;
    loop {
        if i >= 10 {
            return Err(DecodeError { kind: DecodeErrorKind::IntegerOverflow })
        }
        let mut buf = [0u8; 1];
        try!(read_exact(reader, &mut buf));
        num |= (buf[0] as u64 & 0b0111_1111) << (i * 7);
        if buf[0] & 0b1000_0000 == 0 {
            break;
        }
        i += 1;
    }
    Ok(num)
}

fn decode_zig_zag(num: u64) -> i64 {
    if num & 1 == 1 {
        !(num >> 1) as i64
    } else {
        (num >> 1) as i64
    }
}

#[test]
fn test_decode_zig_zag() {
    assert_eq!(decode_zig_zag(0), 0);
    assert_eq!(decode_zig_zag(1), -1);
    assert_eq!(decode_zig_zag(2), 1);
    assert_eq!(decode_zig_zag(3), -2);
    assert_eq!(decode_zig_zag(0xFFFFFFFF_FFFFFFFF), i64::min_value());
    assert_eq!(decode_zig_zag(0xFFFFFFFF_FFFFFFFE), i64::max_value()); // 0x7FFF...FF
}

pub fn decode<R: Read>(reader: &mut R, schema: Schema) -> Result<Value<'static>, DecodeError> {
    match schema {
        Schema::Null => Ok(Value::Null),
        Schema::Boolean => {
            let mut buf = [0u8; 1];
            try!(read_exact(reader, &mut buf[..]));
            match buf[0] {
                0 => Ok(Value::Boolean(false)),
                1 => Ok(Value::Boolean(true)),
                _ => Err(DecodeError { kind: DecodeErrorKind::InvalidBoolean }),
            }
        },
        Schema::Int => {
            let num = decode_zig_zag(try!(decode_var_len_u64(reader)));
            if num < (i32::min_value() as i64) || num > (i32::max_value() as i64) {
                Err(DecodeError { kind: DecodeErrorKind::IntegerOverflow })
            } else {
                Ok(Value::Int(num as i32))
            }
        },
        Schema::Long => {
            Ok(Value::Long(decode_zig_zag(try!(decode_var_len_u64(reader)))))
        },
        Schema::Float => {
            let mut buf = [0u8; 4];
            try!(read_exact(reader, &mut buf[..]));
            Ok(Value::Float(unsafe { mem::transmute(buf) }))
        },
        Schema::Double => {
            let mut buf = [0u8; 8];
            try!(read_exact(reader, &mut buf[..]));
            Ok(Value::Double(unsafe { mem::transmute(buf) }))
        },
        Schema::Bytes => {
            if let Value::Long(len) = try!(decode(reader, Schema::Long)) {
                let mut val: Vec<u8> = repeat(0).take(len as usize).collect();
                try!(read_exact(reader, &mut val));
                Ok(Value::Bytes(Cow::Owned(val)))
            } else {
                panic!("decode returned invalid value");
            }
        },
        Schema::String => {
            if let Value::Long(len) = try!(decode(reader, Schema::Long)) {
                let mut val: Vec<u8> = repeat(0).take(len as usize).collect();
                try!(read_exact(reader, &mut val));
                Ok(Value::String(try!(String::from_utf8(val)).into()))
            } else {
                panic!("decode returned invalid value");
            }
        },
        //Schema::Record(ref inner_schema) => ,
        //Schema::Error(ref inner_schema) => ,
        //Schema::Enum(ref inner_schema) => ,
        //Schema::Array { items } => ,
        //Schema::Map { values } => ,
        //Schema::Union { tys } => ,
        //Schema::Fixed(ref inner_schema) => ,
        _ => unimplemented!(),
    }
}

#[test]
fn test_decode_null() {
    assert_eq!(decode(&mut &b""[..], Schema::Null).unwrap(), Value::Null);
}

#[test]
fn test_decode_boolean() {
    assert_eq!(decode(&mut &b"\x01"[..], Schema::Boolean).unwrap(), Value::Boolean(true));
    assert_eq!(decode(&mut &b"\x00"[..], Schema::Boolean).unwrap(), Value::Boolean(false));
    assert!(decode(&mut &b"\x05"[..], Schema::Boolean).is_err());
}

#[test]
fn test_decode_ints() {
    assert_eq!(decode(&mut &b"\x04"[..], Schema::Int).unwrap(), Value::Int(2));
    assert_eq!(decode(&mut &b"\x84\x02"[..], Schema::Int).unwrap(), Value::Int(130));
    assert_eq!(decode(&mut &b"\x83\x02"[..], Schema::Int).unwrap(), Value::Int(-130));
    assert_eq!(decode(&mut &b"\x84\x02"[..], Schema::Long).unwrap(), Value::Long(130));
}

#[test]
fn test_decode_floats() {
    assert_eq!(decode(&mut &b"\x00\x00\x00\x00"[..], Schema::Float).unwrap(), Value::Float(0.0));
    assert_eq!(decode(&mut &b"\xC3\xF5\x48\x40"[..], Schema::Float).unwrap(), Value::Float(3.14));
    assert_eq!(decode(&mut &b"\x58\x39\xB4\xC8\x76\xBE\x05\x40"[..], Schema::Double).unwrap(),
        Value::Double(2.718));
}

#[test]
fn test_decode_bytes() {
    assert_eq!(decode(&mut &b"\x04\x84\x32"[..], Schema::Bytes).unwrap(),
        Value::Bytes(Cow::Borrowed(&b"\x84\x32"[..])));
}

#[test]
fn test_decode_string() {
    assert_eq!(decode(&mut &b"\x06\x79\x65\x73"[..], Schema::String).unwrap(),
        Value::String("yes".into()));
}