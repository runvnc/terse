// Package testenv is what the reader's tests share with the Rust
// workspace's (crates/terse-testkit): skips that are seen, and a check that
// a reference program is not older than its sources.
//
// Skip records a skip in the workspace's skip log ($TERSE_TEST_SKIPS, else
// target/test-skips.log), which scripts/test-skips.py prints, and with
// TERSE_TEST_NO_SKIP set fails the test instead (unless the need is listed
// in TERSE_TEST_ALLOW_SKIP, comma-separated).
package testenv

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// Refuses tells whether a skip for need is refused, given the values of
// TERSE_TEST_NO_SKIP and TERSE_TEST_ALLOW_SKIP.
func Refuses(noSkip, allow, need string) bool {
	if noSkip == "" || noSkip == "0" {
		return false
	}
	for _, a := range strings.Split(allow, ",") {
		if strings.TrimSpace(a) == need {
			return false
		}
	}
	return true
}

// Skip skips t, which needs need (one word: "programs", "pty", ...), and
// records why; or fails it under TERSE_TEST_NO_SKIP.
func Skip(t testing.TB, need, format string, args ...any) {
	t.Helper()
	why := fmt.Sprintf(format, args...)
	if Refuses(os.Getenv("TERSE_TEST_NO_SKIP"), os.Getenv("TERSE_TEST_ALLOW_SKIP"), need) {
		t.Fatalf("not skipped, as TERSE_TEST_NO_SKIP asks: needs %s: %s", need, why)
	}
	if log := skipLog(); log != "" {
		clean := strings.NewReplacer("\t", " ", "\n", " ", "\r", " ")
		line := fmt.Sprintf("reader-go\t%s\t%s\t%s\n", clean.Replace(t.Name()), need, clean.Replace(why))
		os.MkdirAll(filepath.Dir(log), 0o755)
		if f, err := os.OpenFile(log, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o644); err == nil {
			f.WriteString(line)
			f.Close()
		}
	}
	t.Skipf("[%s] %s", need, why)
}

// Root is the repository's root (the directory above native/reader-go that
// holds Cargo.toml), or "".
func Root() string {
	d, _ := os.Getwd()
	for {
		if _, err := os.Stat(filepath.Join(d, "Cargo.toml")); err == nil {
			if _, err := os.Stat(filepath.Join(d, "native", "reader-go")); err == nil {
				return d
			}
		}
		parent := filepath.Dir(d)
		if parent == d {
			return ""
		}
		d = parent
	}
}

func skipLog() string {
	if l := os.Getenv("TERSE_TEST_SKIPS"); l != "" {
		return l
	}
	root := Root()
	if root == "" {
		return ""
	}
	target := os.Getenv("CARGO_TARGET_DIR")
	if target == "" {
		target = "target"
	}
	if !filepath.IsAbs(target) {
		target = filepath.Join(root, target)
	}
	return filepath.Join(target, "test-skips.log")
}

// Stale returns why the program at path, built by cargo, is older than its
// sources ("" if it is not): cargo writes beside it a dependency file
// (<path>.d, "<path>: <source> <source> ..."), and a source changed after
// the program was built means it no longer corresponds to the sources
// being tested. A program without that file (a copy installed elsewhere)
// cannot be checked: noted is true.
func Stale(path string) (why string, noted bool) {
	exe, err := os.Stat(path)
	if err != nil {
		return err.Error(), false
	}
	dep, err := os.ReadFile(strings.TrimSuffix(path, ".exe") + ".d")
	if err != nil {
		return "", true
	}
	for _, src := range depFiles(string(dep)) {
		st, err := os.Stat(src)
		if err != nil {
			return fmt.Sprintf("%s is gone", src), false
		}
		if st.ModTime().After(exe.ModTime()) {
			return fmt.Sprintf("%s changed after it was built", src), false
		}
	}
	return "", false
}

// depFiles reads the sources of a make-style dependency line ("target: a b
// c", a space in a name escaped with a backslash).
func depFiles(dep string) []string {
	line := strings.SplitN(dep, "\n", 2)[0]
	i := strings.Index(line, ": ")
	if i < 0 {
		return nil
	}
	var files []string
	var cur strings.Builder
	rest := line[i+2:]
	for j := 0; j < len(rest); j++ {
		switch {
		case rest[j] == '\\' && j+1 < len(rest) && rest[j+1] == ' ':
			cur.WriteByte(' ')
			j++
		case rest[j] == ' ':
			if cur.Len() > 0 {
				files = append(files, cur.String())
				cur.Reset()
			}
		default:
			cur.WriteByte(rest[j])
		}
	}
	if cur.Len() > 0 {
		files = append(files, cur.String())
	}
	return files
}
