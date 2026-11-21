//! `terse perf log`: a summary of the performance records (RECORD.md).

use std::path::PathBuf;
use std::process::ExitCode;

/// `terse perf log [<dir or file>]`: the records of the data directory's
/// `perf/` (or `--data <dir>`'s), or those named.
pub fn log(args: &[String]) -> Result<ExitCode, String> {
    let mut path = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--data" => path = Some(PathBuf::from(it.next().ok_or("--data needs a value")?).join("perf")),
            s if !s.starts_with('-') && path.is_none() => path = Some(PathBuf::from(s)),
            _ => return Err("usage: terse perf log [<dir or file> | --data <dir>]".into()),
        }
    }
    let path = match path {
        Some(p) => p,
        None => terse_perflog::record::default_dir().ok_or("no data directory (give one)")?,
    };
    if !path.exists() {
        println!("no records yet in {} (a window's runs write them)", path.display());
        return Ok(ExitCode::SUCCESS);
    }
    print!("{}", terse_perflog::summary::summarise(&path)?);
    Ok(ExitCode::SUCCESS)
}
