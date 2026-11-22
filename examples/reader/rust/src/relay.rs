use std::error::Error;
use std::io::{Read, Write};
use tungstenite::{Message, WebSocket};

pub fn get(
    relay: &mut WebSocket<impl Read + Write>,
    id: [u8; 32],
) -> Result<Vec<u8>, Box<dyn Error>> {
    let request = [&[0x01, 1, 0, 0, 0][..], &id].concat();
    relay.send(Message::binary(request))?;
    loop {
        let reply = relay.read()?.into_data();
        if reply.len() < 6 || reply[0] != 0x81 {
            continue;
        }
        if reply[5] != 1 {
            return Err("the relay does not have it".into());
        }
        if blake3::hash(&reply[6..]) != id {
            return Err("hash check failed".into());
        }
        return Ok(reply[6..].to_vec());
    }
}

pub fn hex32(hex: &str) -> Result<[u8; 32], Box<dyn Error>> {
    if hex.len() != 64 || !hex.is_ascii() {
        return Err("not 64 hexadecimal digits".into());
    }
    let mut bytes = [0; 32];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16)?;
    }
    Ok(bytes)
}
