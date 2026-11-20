// Package vecfile reads the record files of the Terse core test vectors
// (vectors/README): records `<type> <name>` with indented `<field> <value>`
// lines. It is used by the tests only.
package vecfile

import (
	"bufio"
	"encoding/hex"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

// Record is one record of a vector file.
type Record struct {
	Type, Name string
	Fields     [][2]string // in order; a field may repeat
}

// Get returns the first value of a field, and whether it is present.
func (r *Record) Get(f string) (string, bool) {
	for _, kv := range r.Fields {
		if kv[0] == f {
			return kv[1], true
		}
	}
	return "", false
}

// All returns every value of a field, in order.
func (r *Record) All(f string) []string {
	var out []string
	for _, kv := range r.Fields {
		if kv[0] == f {
			out = append(out, kv[1])
		}
	}
	return out
}

// Dir finds the repository's vectors directory by walking up from the
// working directory.
func Dir() (string, error) {
	d, err := os.Getwd()
	if err != nil {
		return "", err
	}
	for {
		p := filepath.Join(d, "vectors", "README")
		if _, err := os.Stat(p); err == nil {
			return filepath.Join(d, "vectors"), nil
		}
		parent := filepath.Dir(d)
		if parent == d {
			return "", fmt.Errorf("vectors directory not found")
		}
		d = parent
	}
}

// Parse reads a record file.
func Parse(path string) ([]*Record, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	var recs []*Record
	var cur *Record
	sc := bufio.NewScanner(f)
	sc.Buffer(make([]byte, 1<<20), 1<<24)
	for sc.Scan() {
		line := sc.Text()
		if strings.HasPrefix(line, "#") {
			continue
		}
		if strings.TrimSpace(line) == "" {
			cur = nil
			continue
		}
		if strings.HasPrefix(line, "  ") {
			if cur == nil {
				return nil, fmt.Errorf("field outside a record: %q", line)
			}
			body := line[2:]
			k, v, _ := strings.Cut(body, " ")
			cur.Fields = append(cur.Fields, [2]string{k, v})
			continue
		}
		t, n, _ := strings.Cut(line, " ")
		cur = &Record{Type: t, Name: n}
		recs = append(recs, cur)
	}
	return recs, sc.Err()
}

// Hex decodes a hex field; "-" is no bytes.
func Hex(s string) ([]byte, error) {
	if s == "-" {
		return nil, nil
	}
	return hex.DecodeString(s)
}

// Pattern returns n bytes of the mod251 pattern.
func Pattern(n int) []byte {
	b := make([]byte, n)
	for i := range b {
		b[i] = byte(i % 251)
	}
	return b
}

// Unquote decodes a quoted string of the vectors.
func Unquote(s string) (string, error) {
	if len(s) < 2 || s[0] != '"' || s[len(s)-1] != '"' {
		return "", fmt.Errorf("not quoted: %q", s)
	}
	s = s[1 : len(s)-1]
	var b strings.Builder
	for i := 0; i < len(s); i++ {
		c := s[i]
		if c != '\\' {
			b.WriteByte(c)
			continue
		}
		i++
		if i >= len(s) {
			return "", fmt.Errorf("trailing backslash")
		}
		switch s[i] {
		case '\\':
			b.WriteByte('\\')
		case '"':
			b.WriteByte('"')
		case 't':
			b.WriteByte('\t')
		case 'n':
			b.WriteByte('\n')
		case 'r':
			b.WriteByte('\r')
		default:
			return "", fmt.Errorf("bad escape %q", s[i])
		}
	}
	return b.String(), nil
}

// Atoi parses a decimal field.
func Atoi(s string) int {
	n, _ := strconv.Atoi(s)
	return n
}
