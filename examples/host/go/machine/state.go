package machine

func (m *Machine) State(id [32]byte,
	grants, terminal []byte) []byte {
	state := append([]byte("terse-state-1"), id[:]...)
	state = le.AppendUint32(state, m.PC)
	for _, x := range m.X[1:] {
		state = le.AppendUint32(state, x)
	}
	state = le.AppendUint32(state, m.Frame)
	fault, faultPC := Fault{}, uint32(0)
	if m.Fault != nil {
		fault, faultPC = *m.Fault, m.PC
	}
	state = append(state, m.Stopped, m.ExitCode, fault.Kind)
	state = le.AppendUint32(state, fault.Encoding)
	state = le.AppendUint32(state, faultPC)
	state = le.AppendUint32(state, fault.Address)
	state = le.AppendUint32(state, uint32(m.Budget))
	state = le.AppendUint32(state, uint32(len(m.RAM)))
	state = append(append(state, m.RAM...), grants...)
	state = append(state, make([]byte, 37)...)
	if terminal == nil {
		return append(state, 0, 0, 0, 0)
	}
	state = append(append(state, 1), terminal...)
	return append(state, 0, 0, 0)
}
