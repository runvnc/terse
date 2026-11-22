# tersenet (Rust)

Checks Tersenet feed entries: every validity rule of NETWORK.md 4.2, with
the strict Ed25519 verification the network requires (ordinary Ed25519
libraries accept signatures it refuses). One file, `src/lib.rs`;
`curve25519-dalek` and `sha2`; `no_std`, no allocator, no options, no
state, no I/O.

    cargo add tersenet

```rust
let e = tersenet::verify_entry(&bytes)?; // Err names the rule
println!("{} {}", e.seq, e.title);
```

Also `verify(key, message, signature) -> bool` (strict Ed25519 for
anything else) and `valid_feed_key(key) -> bool` (NETWORK.md 4.1).

The rules and the reasons for them: the manual's "Signatures and valid
entries" (docs/manual/signatures.txt in the repository). Tests:
`cargo test -p tersenet` (the entries of vectors/entries.txt and the
generated cases of ../testdata/strict.txt, also checked against
ed25519-dalek).

Licence: Apache-2.0.
