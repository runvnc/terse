// Step 2: fetch an article from a relay by its ID.
use reader::{article, relay};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let id = relay::hex32(&args[2])?;
    let (mut socket, _) = tungstenite::connect(&args[1])?;
    let blob = relay::get(&mut socket, id)?;
    if blob.starts_with(b"TERS") || blob.starts_with(b"TDOC") {
        return Err("not an article".into());
    }
    article::show(&blob);
    Ok(())
}
