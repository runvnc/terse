//go:build linux || darwin || freebsd || netbsd || openbsd

package main

import (
	"syscall"
	"unsafe"
)

// Raw terminal mode through ioctl, with no library.

type termState struct{ t syscall.Termios }

func ioctl(fd int, req uintptr, arg unsafe.Pointer) error {
	_, _, e := syscall.Syscall(syscall.SYS_IOCTL, uintptr(fd), req, uintptr(arg))
	if e != 0 {
		return e
	}
	return nil
}

func isTerminal(fd int) bool {
	var t syscall.Termios
	return ioctl(fd, uintptr(ioctlGetTermios), unsafe.Pointer(&t)) == nil
}

func makeRaw(fd int) (*termState, error) {
	var old syscall.Termios
	if err := ioctl(fd, uintptr(ioctlGetTermios), unsafe.Pointer(&old)); err != nil {
		return nil, err
	}
	raw := old
	raw.Iflag &^= syscall.IGNBRK | syscall.BRKINT | syscall.PARMRK | syscall.ISTRIP |
		syscall.INLCR | syscall.IGNCR | syscall.ICRNL | syscall.IXON
	raw.Oflag &^= syscall.OPOST
	raw.Lflag &^= syscall.ECHO | syscall.ECHONL | syscall.ICANON | syscall.ISIG | syscall.IEXTEN
	raw.Cflag &^= syscall.CSIZE | syscall.PARENB
	raw.Cflag |= syscall.CS8
	raw.Cc[syscall.VMIN] = 1
	raw.Cc[syscall.VTIME] = 0
	if err := ioctl(fd, uintptr(ioctlSetTermios), unsafe.Pointer(&raw)); err != nil {
		return nil, err
	}
	return &termState{old}, nil
}

func restore(fd int, s *termState) {
	ioctl(fd, uintptr(ioctlSetTermios), unsafe.Pointer(&s.t))
}

func termSize(fd int) (cols, rows int) {
	var ws struct{ Row, Col, X, Y uint16 }
	if ioctl(fd, uintptr(syscall.TIOCGWINSZ), unsafe.Pointer(&ws)) != nil || ws.Col == 0 {
		return 80, 24
	}
	return int(ws.Col), int(ws.Row)
}
