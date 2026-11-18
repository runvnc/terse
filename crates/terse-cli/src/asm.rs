//! Z80 assembly through the `rsasm` library (Zilog syntax, 8-bit dialect).

/// Assembles `src` into a flat binary. `include` files are looked up in the
/// directory of `name`.
pub fn assemble(name: &str, src: &str) -> Result<Vec<u8>, String> {
    let arch = rsasm::arch::lookup("z80").ok_or("rsasm was built without the z80 backend")?;
    let dir = std::path::Path::new(name).parent().unwrap_or(std::path::Path::new(""));
    let options = rsasm::assembler::Options::new()
        .with_dialect(arch.default_dialect())
        .with_relocatable(false)
        .with_include_path(if dir.as_os_str().is_empty() { std::path::Path::new(".") } else { dir });
    let mut asm = rsasm::assembler::Assembler::new(arch, options);
    let src: String = src.lines().map(|l| protect_symbol_operands(l) + "\n").collect();
    asm.assemble_str(name, &src);
    let ok = asm.finish();
    if !ok || asm.diags().has_errors() {
        return Err(asm.diags().render(asm.source_map(), false));
    }
    // The image starts at the lowest address assembled, so sources begin with ORG 0.
    rsasm::output::raw::build(&asm).map_err(|e| e.to_string())
}

const REGISTERS: [&str; 16] =
    ["a", "b", "c", "d", "e", "h", "l", "i", "r", "af", "bc", "de", "hl", "sp", "ix", "iy"];

/// Works around rsasm 0.1.3 rejecting a parenthesised lone symbol such as
/// `ld a,(var)` or `out (PORT),a` (it takes every lone name in parentheses
/// for a register): rewrites `(name)` to `(0+name)` unless `name` is a register.
/// Strings and comments are left alone.
fn protect_symbol_operands(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + 8);
    let mut quote: Option<char> = None;
    let mut rest = line;
    while let Some(c) = rest.chars().next() {
        rest = &rest[c.len_utf8()..];
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            // `af'` is a register name, not the start of a string.
            None if c == '\'' && out.to_ascii_lowercase().ends_with("af") => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == ';' => {
                out.push(c);
                out.push_str(rest);
                return out;
            }
            None if c == '(' => {
                let len = rest
                    .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
                    .unwrap_or(rest.len());
                let ident = &rest[..len];
                let is_symbol = ident.starts_with(|ch: char| ch.is_ascii_alphabetic() || ch == '_')
                    && !REGISTERS.contains(&ident.to_ascii_lowercase().as_str());
                if is_symbol && rest[len..].starts_with(')') {
                    out.push_str("(0+");
                    continue;
                }
            }
            None => {}
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_only_lone_symbols() {
        assert_eq!(protect_symbol_operands(" out (PORT),a"), " out (0+PORT),a");
        assert_eq!(protect_symbol_operands(" ld a,(var_1)"), " ld a,(0+var_1)");
        assert_eq!(protect_symbol_operands(" ld a,(hl)"), " ld a,(hl)");
        assert_eq!(protect_symbol_operands(" in a,(C)"), " in a,(C)");
        assert_eq!(protect_symbol_operands(" ld a,(ix+1)"), " ld a,(ix+1)");
        assert_eq!(protect_symbol_operands(" ld a,(1234h)"), " ld a,(1234h)");
        assert_eq!(protect_symbol_operands(" db \"(x)\" ; (y)"), " db \"(x)\" ; (y)");
        assert_eq!(protect_symbol_operands(" ex af,af' ; (y)"), " ex af,af' ; (y)");
    }

    #[test]
    fn assembles_symbolic_ports_and_utf8() {
        let bin = assemble("t", " org 0\nP equ 20h\n out (P),a\n ld a,(V)\nV db \"é\"\n").unwrap();
        assert_eq!(bin, [0xD3, 0x20, 0x3A, 0x05, 0x00, 0xC3, 0xA9]);
    }
}
