package main

import (
	"fmt"
	"os"

	"host/machine"
)

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
		if ecall && cpu.X[17] == 0x106 {
			os.Exit(int(cpu.X[10] & 0xff))
		}
		if ecall {
			cpu.X[10] = 0xffffffff
		}
	}
}
