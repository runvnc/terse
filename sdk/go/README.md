# tersenet (Go)

Checks Tersenet feed entries: every validity rule of NETWORK.md 4.2, with
the strict Ed25519 verification the network requires (Go's
`crypto/ed25519.Verify` accepts signatures it refuses). One file,
`tersenet.go`; the standard library and `filippo.io/edwards25519`. No
options, no state, no I/O.

    go get github.com/runvnc/terse/sdk/go@v0.9.0

```go
import tersenet "github.com/runvnc/terse/sdk/go"

e, err := tersenet.VerifyEntry(b) // b: an entry as received
if err != nil { return err }      // names the rule: tersenet.ErrKey, ...
fmt.Println(e.Seq, e.Title)
```

Also `Verify(key, message, signature) bool` (strict Ed25519 for anything
else) and `ValidFeedKey(key) bool` (NETWORK.md 4.1).

The rules and the reasons for them: the manual's "Signatures and valid
entries" (docs/manual/signatures.txt in the repository). Tests:
`go test ./...` (the entries of vectors/entries.txt and the cases of
../testdata/strict.txt); `go test -fuzz FuzzVerifyEntry`.

Licence: Apache-2.0.
