// Step 3: follow a feed and open one of its articles.
use reader::{article, relay};
use std::collections::HashMap;
use tungstenite::Message;

const TYPE_SUBSCRIBE: u8 = 0x04;
const TYPE_ENTRY: u8 = 0x90;
const REQUEST_NUMBER: u32 = 2;
const AFTER: u32 = 0;
const STATUS_AT: usize = 5;
const STATUS_DONE: u8 = 1;
const HIGHEST_AT: usize = 6;
const HIGHEST_END: usize = 10;
const SEQ_AT: usize = 32;
const SEQ_END: usize = 36;
const KIND_ARTICLE: u8 = 1;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let key = relay::hex32(&args[2])?;
    let (mut socket, _) = tungstenite::connect(&args[1])?;
    let number = REQUEST_NUMBER.to_le_bytes();
    let after = AFTER.to_le_bytes();
    let request =
        [&[TYPE_SUBSCRIBE][..], &number, &key, &after].concat();
    socket.send(Message::binary(request))?;
    let reply = socket.read()?.into_data();
    if reply.len() < HIGHEST_END
        || reply[STATUS_AT] != STATUS_DONE
    {
        return Err("the relay refused the subscription".into());
    }
    let highest = reply[HIGHEST_AT..HIGHEST_END].try_into()?;
    let highest = u32::from_le_bytes(highest);
    let mut articles = HashMap::new();
    let mut caught_up = highest == AFTER;
    while !caught_up {
        let msg = socket.read()?.into_data();
        let Some(entry) = msg.strip_prefix(&[TYPE_ENTRY]) else {
            continue;
        };
        if entry.len() < SEQ_END {
            continue;
        }
        let seq = entry[SEQ_AT..SEQ_END].try_into()?;
        let seq = u32::from_le_bytes(seq);
        caught_up = seq >= highest;
        let Ok(e) = tersenet::verify_entry(entry) else {
            continue;
        };
        if e.feed == key && e.kind == KIND_ARTICLE {
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
