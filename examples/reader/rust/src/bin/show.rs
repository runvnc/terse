// Step 1: show an article from a file.
fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    reader::article::show(&std::fs::read(&args[1])?);
    Ok(())
}
