package main

import (
	"fmt"
	"os"
	"strconv"

	"host/machine"
)

func main() {
	file, err := os.ReadFile(os.Args[1])
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
	frames, _ := strconv.Atoi(os.Args[2])
	cpu, _, err := machine.Load(file)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
	m := &machine.Machine{CPU: cpu}
	for m.Stopped == machine.Running && int(m.Frame) < frames {
		m.RunFrame()
	}
	if m.Fault != nil {
		fmt.Fprintln(os.Stderr, m.Fault)
		os.Exit(1)
	}
	os.Exit(int(m.ExitCode))
}
