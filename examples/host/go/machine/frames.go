package machine

const FrameBudget = 125000

const (
	Running = iota
	Exited
	Faulted
)

type Machine struct {
	*CPU
	Budget   int64
	Frame    uint32
	Stopped  byte
	ExitCode byte
	Fault    *Fault
	Devices  func(slot, fn uint32) (result uint32, charge int64)
	waiting  bool
}

func (m *Machine) RunFrame() {
	m.Budget += FrameBudget
	m.waiting = false
	for m.Budget > 0 && m.Stopped == Running && !m.waiting {
		cost, ecall, fault := m.Step()
		if fault != nil {
			m.Stopped, m.Fault = Faulted, fault
			break
		}
		m.Budget -= cost
		if ecall {
			m.call()
		}
	}
	m.Budget = min(m.Budget, 0)
	m.Frame++
}

func (m *Machine) call() {
	slot, fn := m.X[17]>>8, m.X[17]&0xff
	result, charge := uint32(0xffffffff), int64(4)
	if slot == 1 {
		result = m.system(fn)
	} else if m.Devices != nil {
		result, charge = m.Devices(slot, fn)
	}
	m.X[10] = result
	m.Budget -= charge
}

func (m *Machine) system(fn uint32) uint32 {
	switch fn {
	case 0:
		m.waiting = true
		return m.Frame + 1
	case 1:
		if m.Memory(m.X[10], 4, true) == nil {
			return 0xfffffffe
		}
		return 0
	case 2:
		return m.Frame
	case 6:
		m.Stopped, m.ExitCode = Exited, byte(m.X[10])
		return 0
	}
	return 0xffffffff
}
