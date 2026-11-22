package article

import (
	"fmt"
	"strings"
	"unicode"
	"unicode/utf8"
)

const width = 68

func Show(text []byte) {
	s := strings.ToValidUTF8(string(text), "\uFFFD")
	s = strings.TrimPrefix(s, "\uFEFF")
	var paragraph []string
	end := func() {
		if paragraph != nil {
			block(strings.Join(paragraph, " "), "")
			paragraph = nil
		}
	}
	for _, line := range strings.Split(s, "\n") {
		switch {
		case strings.Trim(line, " \t\r") == "":
			end()
		case strings.HasPrefix(line, "# "):
			end()
			block(line[2:], "=")
		case strings.HasPrefix(line, "## "):
			end()
			block(line[3:], "-")
		default:
			paragraph = append(paragraph, line)
		}
	}
	end()
}

func block(text, rule string) {
	lines := wrap(text)
	if rule != "" {
		n := utf8.RuneCountInString(strings.Join(lines, " "))
		underline := strings.Repeat(rule, min(n, width))
		lines = append(lines, underline)
	}
	fmt.Printf("%s\n\n", strings.Join(lines, "\n"))
}

func wrap(text string) []string {
	var lines []string
	line := ""
	for _, word := range strings.Split(Visible(text), " ") {
		switch {
		case word == "":
		case line == "":
			line = word
		case utf8.RuneCountInString(line+" "+word) > width:
			lines = append(lines, line)
			line = word
		default:
			line += " " + word
		}
	}
	return append(lines, line)
}

func Visible(text string) string {
	return strings.Map(func(r rune) rune {
		switch {
		case r == '\t' || r == '\r':
			return ' '
		case unicode.IsControl(r):
			return -1
		}
		return r
	}, text)
}
