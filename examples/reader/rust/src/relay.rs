use std::error::Error;
use std::io::{Read, Write};
use tungstenite::{Message, WebSocket};

pub const ID_SIZE: usize = 32;
pub type Id = [u8; ID_SIZE];

const TYPE_GET: u8 = 0x01;
const TYPE_GET_REPLY: u8 = 0x81;
const REQUEST_NUMBER: u32 = 1;
const STATUS_AT: usize = 5;
const BLOB_AT: usize = 6;
const STATUS_FOUND: u8 = 1;
const STATUS_NOT_FOUND: u8 = 2;

pub fn get(
    relay: &mut WebSocket<impl Read + Write>,
    id: Id,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let number = REQUEST_NUMBER.to_le_bytes();
    let request = [&[TYPE_GET][..], &number, &id].concat();
    relay.send(Message::binary(request))?;
    loop {
        let reply = relay.read()?.into_data();
        if reply.len() < BLOB_AT || reply[0] != TYPE_GET_REPLY {
            continue;
        }
        match reply[STATUS_AT] {
            STATUS_FOUND => {}
            STATUS_NOT_FOUND => {
                return Err("the relay does not have it".into());
            }
            _ => return Err("the relay refused it".into()),
        }
        if blake3::hash(&reply[BLOB_AT..]) != id {
            return Err("hash check failed".into());
        }
        return Ok(reply[BLOB_AT..].to_vec());
    }
}

pub fn hex32(hex: &str) -> Result<Id, Box<dyn Error>> {
    if hex.len() != 2 * ID_SIZE || !hex.is_ascii() {
        return Err("not 64 hexadecimal digits".into());
    }
    let mut bytes = [0; ID_SIZE];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16)?;
    }
    Ok(bytes)
}
