// Step 3: follow a feed and open one of its articles.
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
        if message.len() < 37 || message[0] != 0x90 {
            continue;
        }
        let entry = &message[1..];
        let seq = u32::from_le_bytes(entry[32..36].try_into()?);
        caught_up = seq >= highest;
        let Ok(e) = tersenet::verify_entry(entry) else {
            continue;
        };
        if e.feed == key && e.kind == 1 {
            println!("{seq}  {}", article::visible(e.title));
            articles.insert(seq, e.blob);
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
