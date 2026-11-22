package main

import (
	"encoding/binary"
	"fmt"
	"os"
	"strconv"
	"strings"

	"host/machine"
)

func main() {
	file, err := os.ReadFile(os.Args[1])
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
	frames, _ := strconv.Atoi(os.Args[2])
	cpu, devices, err := machine.Load(file)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
	m := &machine.Machine{CPU: cpu}
	bus := machine.Start(os.Args[3], m, file, devices)
	m.Devices = bus.Call
	for m.Stopped == machine.Running && int(m.Frame) < frames {
		m.Budget -= bus.Begin(m.Frame)
		m.RunFrame()
		bus.End()
	}
	fmt.Print(screen(bus.State(machine.Terminal)))
	bus.Close()
	if m.Fault != nil {
		fmt.Fprintln(os.Stderr, m.Fault)
		os.Exit(1)
	}
	os.Exit(int(m.ExitCode))
}

const (
	columns      = 80
	rows         = 30
	cellSize     = 4
	continuation = 0xffff
)

func screen(grid []byte) string {
	code := func(cell int) rune {
		at := cellSize * cell
		return rune(binary.LittleEndian.Uint16(grid[at:]))
	}
	var text strings.Builder
	for row := range rows {
		var line []rune
		for col := range columns {
			cell := row*columns + col
			rightHalf := col > 0 && code(cell-1) != continuation
			switch {
			case code(cell) != continuation:
				line = append(line, code(cell))
			case !rightHalf:
				line = append(line, ' ')
			}
		}
		trimmed := strings.TrimRight(string(line), " ")
		text.WriteString(trimmed + "\n")
	}
	return text.String()
}
