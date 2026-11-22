// Step 3: follow a feed and open one of its articles.
use ed25519_dalek::{Signature, VerifyingKey};
use reader::{article, relay};
use std::collections::HashMap;
use tungstenite::Message;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let key = relay::hex32(&args[2])?;
    let (mut socket, _) = tungstenite::connect(&args[1])?;
    let subscribe = [&[0x04, 2, 0, 0, 0][..], &key, &[0; 4]];
    socket.send(Message::binary(subscribe.concat()))?;
    let reply = socket.read()?.into_data();
    if reply.len() < 10 || reply[5] != 1 {
        return Err("the relay refused the subscription".into());
    }
    let highest = u32::from_le_bytes(reply[6..10].try_into()?);
    let mut articles = HashMap::new();
    let mut caught_up = highest == 0;
    while !caught_up {
        let message = socket.read()?.into_data();
        if message.len() < 41 || message[0] != 0x90 {
            continue;
        }
        let entry = &message[1..];
        let seq = u32::from_le_bytes(entry[32..36].try_into()?);
        caught_up = seq >= highest;
        if entry[40] == 1 && valid(entry, &key) {
            let title = &entry[109..entry.len() - 64];
            let title = String::from_utf8_lossy(title);
            println!("{seq}  {}", article::visible(&title));
            articles.insert(seq, entry[44..76].try_into()?);
        }
    }
    let Some(seq) = args.get(3) else { return Ok(()) };
    let Some(&id) = articles.get(&seq.parse::<u32>()?) else {
        return Err("no such article in the feed".into());
    };
    let blob = relay::get(&mut socket, id)?;
    println!();
    article::show(&blob);
    Ok(())
}

fn valid(e: &[u8], key: &[u8; 32]) -> bool {
    if e.len() < 173 || e.len() != 173 + e[108] as usize {
        return false;
    }
    let (signed, signature) = e.split_at(e.len() - 64);
    let message = [b"terse-entry-0", signed].concat();
    let seq = u32::from_le_bytes([e[32], e[33], e[34], e[35]]);
    let size = u32::from_le_bytes([e[41], e[42], e[43], 0]);
    e[..32] == key[..]
        && e[108] <= 120
        && seq >= 1
        && (1..=1 << 20).contains(&size)
        && std::str::from_utf8(&signed[109..]).is_ok()
        && verify(key, &message, signature)
}

fn verify(key: &[u8; 32], message: &[u8], sig: &[u8]) -> bool {
    let sig = Signature::from_slice(sig).unwrap();
    VerifyingKey::from_bytes(key).is_ok_and(|feed| {
        feed.to_edwards().compress().0 == *key
            && feed.verify_strict(message, &sig).is_ok()
    })
}
