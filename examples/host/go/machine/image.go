package machine

import "errors"

type Device struct {
	Slot, Version byte
	Optional      bool
}

func Load(file []byte) (*CPU, []Device, error) {
	if len(file) < 32 || string(file[:6]) != "TERS\x00\x01" {
		return nil, nil, errors.New("not a Terse32 program")
	}
	romLen := int(file[6]) | int(file[7])<<8 | int(file[8])<<16
	pages, count := int(file[9]), int(file[10])
	size := 32 + 2*count + romLen
	if romLen < 1 || romLen > 131072 || pages < 1 ||
		pages > 128 || count > 16 || len(file) != size {
		return nil, nil, errors.New("a malformed program file")
	}
	var devices []Device
	for i := 32; i < 32+2*count; i += 2 {
		version := file[i+1]
		devices = append(devices,
			Device{file[i], version & 0x7f, version&0x80 != 0})
	}
	cpu := &CPU{PC: ROMBase, ROM: file[32+2*count:],
		RAM: make([]byte, pages*8192)}
	cpu.X[2] = RAMBase + uint32(len(cpu.RAM))
	return cpu, devices, nil
}
