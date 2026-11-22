package main

import (
	"fmt"
	"os"
	"strconv"

	"host/machine"

	"lukechampine.com/blake3"
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
	var bus *machine.Bus
	if len(devices) > 0 {
		bus = machine.Start(os.Args[3], m, file, devices)
		m.Devices = bus.Call
	}
	for m.Stopped == machine.Running && int(m.Frame) < frames {
		if bus != nil {
			m.Budget -= bus.Begin(m.Frame)
		}
		m.RunFrame()
		if bus != nil {
			bus.End()
		}
	}
	var grants, terminal []byte
	if bus != nil {
		grants, terminal = bus.Grants, bus.State(2)
		bus.Close()
	}
	state := m.State(blake3.Sum256(file), grants, terminal)
	fmt.Printf("%x\n", blake3.Sum256(state))
	if m.Fault != nil {
		fmt.Fprintln(os.Stderr, m.Fault)
		os.Exit(1)
	}
	os.Exit(int(m.ExitCode))
}
