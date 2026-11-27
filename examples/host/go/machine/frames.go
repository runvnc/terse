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
}

const (
	System     = 1
	BaseCharge = 4
	BadBuffer  = 0xfffffffe
	eventSize  = 4
)

const (
	waitFrame  = 0
	nextEvent  = 1
	frameCount = 2
	exit       = 6
	copyText   = 10
	maxCopy    = 65536
)

func (m *Machine) RunFrame() {
	m.Budget += FrameBudget
	for m.Budget > 0 && m.Stopped == Running {
		cost, ecall, fault := m.Step()
		if fault != nil {
			m.Stopped, m.Fault = Faulted, fault
			break
		}
		m.Budget -= cost
		if !ecall {
			continue
		}
		slot, fn := m.X[A7]>>8, m.X[A7]&0xff
		result, charge := uint32(UnknownCall), int64(BaseCharge)
		if slot == System {
			result, charge = m.system(fn)
		} else if m.Devices != nil {
			result, charge = m.Devices(slot, fn)
		}
		m.X[A0] = result
		m.Budget -= charge
		if slot == System && fn == waitFrame {
			break
		}
	}
	m.Budget = min(m.Budget, 0)
	m.Frame++
}

func (m *Machine) system(fn uint32) (uint32, int64) {
	switch fn {
	case waitFrame:
		return m.Frame + 1, BaseCharge
	case nextEvent:
		if m.Memory(m.X[A0], eventSize, true) == nil {
			return BadBuffer, BaseCharge
		}
		return 0, BaseCharge
	case frameCount:
		return m.Frame, BaseCharge
	case exit:
		m.Stopped, m.ExitCode = Exited, byte(m.X[A0])
		return 0, BaseCharge
	case copyText:
		// No clipboard: check, charge, drop.
		n := m.X[A1]
		text := m.Memory(m.X[A0], n, false)
		if n > maxCopy || (n > 0 && text == nil) {
			return BadBuffer, BaseCharge
		}
		return 0, BaseCharge + int64(n+3)/4
	}
	return UnknownCall, BaseCharge
}
