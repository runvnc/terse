//go:build !thin

package terse

import (
	"encoding/hex"
	"os"
	"strings"
	"testing"
)

// TestCorpus writes ParseEntry's verdict (valid or invalid) on each line
// (hex) of the file TERSENET_CORPUS to TERSENET_VERDICTS. The workspace's
// crates/terse-cli/tests/sdk_agreement.rs runs it to compare this reader,
// written from the specifications alone, with the reference and the SDKs
// on thousands of altered entries.
func TestCorpus(t *testing.T) {
	in, out := os.Getenv("TERSENET_CORPUS"), os.Getenv("TERSENET_VERDICTS")
	if in == "" {
		t.Skip("TERSENET_CORPUS not set: run by crates/terse-cli/tests/sdk_agreement.rs")
	}
	text, err := os.ReadFile(in)
	if err != nil {
		t.Fatal(err)
	}
	var verdicts strings.Builder
	for _, line := range strings.Fields(string(text)) {
		b, err := hex.DecodeString(line)
		if err != nil {
			t.Fatal(err)
		}
		if _, err := ParseEntry(b); err != nil {
			verdicts.WriteString("invalid\n")
		} else {
			verdicts.WriteString("valid\n")
		}
	}
	if err := os.WriteFile(out, []byte(verdicts.String()), 0o644); err != nil {
		t.Fatal(err)
	}
}
