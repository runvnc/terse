// Step 1: show an article from a file.
package main

import (
	"log"
	"os"

	"reader/article"
)

func main() {
	text, err := os.ReadFile(os.Args[1])
	if err != nil {
		log.Fatal(err)
	}
	article.Show(text)
}
