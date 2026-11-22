package main

import (
	"fmt"
	"os"

	"host/machine"
)

const exit = 0x106

func main() {
	file, err := os.ReadFile(os.Args[1])
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
	cpu, _, err := machine.Load(file)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
	for {
		_, ecall, fault := cpu.Step()
		if fault != nil {
			fmt.Fprintln(os.Stderr, fault)
			os.Exit(1)
		}
		if ecall && cpu.X[machine.A7] == exit {
			os.Exit(int(byte(cpu.X[machine.A0])))
		}
		if ecall {
			cpu.X[machine.A0] = machine.UnknownCall
		}
	}
}
