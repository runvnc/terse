package machine

const (
	stateTag    = "terse-state-1"
	emptySystem = 37
	absent      = 0
	present     = 1
)

func (m *Machine) State(id [32]byte,
	grants, terminal []byte) []byte {
	state := append([]byte(stateTag), id[:]...)
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
	state = append(state, make([]byte, emptySystem)...)
	if terminal == nil {
		state = append(state, absent)
	} else {
		state = append(append(state, present), terminal...)
	}
	return append(state, absent, absent, absent)
}
