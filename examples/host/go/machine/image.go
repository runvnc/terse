package machine

import (
	"errors"
	"strings"
)

const (
	headerSize  = 32
	magic       = "TERS\x00\x01"
	romLenAt    = 6
	ramPagesAt  = 9
	devicesAt   = 10
	maxROM      = 131072
	maxRAMPages = 128
	maxDevices  = 16
	deviceSize  = 2
	PageSize    = 8192
	optional    = 0x80
)

type Device struct {
	Slot, Version byte
	Optional      bool
}

func Load(file []byte) (*CPU, []Device, error) {
	if len(file) < headerSize ||
		!strings.HasPrefix(string(file), magic) {
		return nil, nil, errors.New("not a Terse32 program")
	}
	r := file[romLenAt:]
	romLen := int(r[0]) | int(r[1])<<8 | int(r[2])<<16
	pages, count := int(file[ramPagesAt]), int(file[devicesAt])
	romAt := headerSize + deviceSize*count
	if romLen < 1 || romLen > maxROM || pages < 1 ||
		pages > maxRAMPages || count > maxDevices ||
		len(file) != romAt+romLen {
		return nil, nil, errors.New("a malformed program file")
	}
	var devices []Device
	for i := headerSize; i < romAt; i += deviceSize {
		version := file[i+1]
		devices = append(devices, Device{file[i],
			version &^ optional, version&optional != 0})
	}
	cpu := &CPU{PC: ROMBase, ROM: file[romAt:],
		RAM: make([]byte, pages*PageSize)}
	cpu.X[SP] = RAMBase + uint32(len(cpu.RAM))
	return cpu, devices, nil
}
