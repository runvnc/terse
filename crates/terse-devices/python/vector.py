#!/usr/bin/env python3
"""Terse device 3, the 2D vector display (SPEC.md 10, version 0), as a
device driver of the device bus (DEVICES.md), in Python 3 with the
standard library only. It draws nothing (no layers); it keeps the device
state (STATE.md 1.7) and reports the figures-as-outlines flag, property
key 1, when presenting (DEVICES.md 7). Socket transport only.

    TERSE_BUS_SOCKET=<path> TERSE_BUS_SLOT=3 vector.py
"""
import os
import socket
import struct
import sys

MAX_LIST = 16384


class Vector:
    """The device state (SPEC 10.7): style, persistence, fill mode, list."""

    def __init__(self):
        self.style, self.persistence, self.outlines, self.list = 0, 160, 0, b""

    def call(self, fn, a, data):
        """Carries out a call; returns (a0, charge on top of the 4)."""
        if fn == 0:  # submit (the core has checked the length and place)
            if not 1 <= a[1] <= MAX_LIST or len(data) != a[1]:
                return 1, 0
            self.list = bytes(data)
            return 0, (len(data) + 3) // 4
        if fn == 1:  # clear
            self.list = b""
            return 0, 0
        if fn == 2 and a[0] <= 1:  # style
            self.style = a[0]
            return 0, 0
        if fn == 3 and a[0] <= 255:  # persistence
            self.persistence = a[0]
            return 0, 0
        if fn == 4 and a[0] <= 1:  # fill
            self.outlines = a[0]
            return 0, 0
        return (1, 0) if fn in (2, 3, 4) else (0xFFFFFFFF, 0)

    def state(self):  # STATE.md 1.7, after the present flag
        return bytes([self.style, self.persistence, self.outlines]) + struct.pack("<I", len(self.list)) + self.list

    def restore(self, s):
        if len(s) < 7 or s[0] > 1 or s[2] > 1:
            return False
        m = struct.unpack_from("<I", s, 3)[0]
        if m > MAX_LIST or len(s) != 7 + m:
            return False
        self.style, self.persistence, self.outlines, self.list = s[0], s[1], s[2], bytes(s[7:])
        return True


def entry(fn, rule, value=0, submit=False):
    """A 64-byte call table entry (DEVICES.md 5): batchable, with a result
    rule; submit's buffer is a0, a1 x 1, 1-16,384 bytes, read, RAM only,
    both failures 1, charged 1 per 4 bytes."""
    if submit:
        buf = (0x06, 0, 1, 1, 1, MAX_LIST, 0x01, 0xFF, 0, 1, 1)
    else:
        buf = (0x02, 0, 0xFF, 0, 0, 0, 0, 0xFF, 0, 0, 0)
    flags, addr, length, scale, least, greatest, access, checked, cmax, lres, pres = buf
    return struct.pack("<BBBBIIIBBHIII", fn, flags, addr, length, scale, least, greatest,
                       access, checked, 0, cmax, lres, pres) + \
        struct.pack("<BBHIII", rule, 0, 0, value, 0, 0) + \
        struct.pack("<BBHIII", 1 if submit else 0, 0xFF, 0, 0, 0, 0)


TABLE = [entry(0, 1, submit=True), entry(1, 1), entry(2, 3, 1), entry(3, 3, 255), entry(4, 3, 1)]


class Bus:
    def __init__(self, path):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(path)

    def recv(self):
        head = self.exact(4)
        return self.exact(struct.unpack("<I", head)[0]) if head else None

    def exact(self, n):
        out = bytearray()
        while len(out) < n:
            chunk = self.sock.recv(n - len(out))
            if not chunk:
                return None
            out += chunk
        return bytes(out)

    def send(self, body):
        self.sock.sendall(struct.pack("<I", len(body)) + body)


def call(dev, m):
    fn = m[1]
    a = struct.unpack_from("<6I", m, 4)
    n = struct.unpack_from("<I", m, 36)[0]
    a0, charge = dev.call(fn, a, m[40:40 + n])
    return struct.pack("<BBHIIII", 0x90, 0, 0, a0, 0, charge, 0)


def main():
    if os.environ.get("TERSE_BUS_SLOT") != "3":
        sys.exit("vector.py: this is the driver of slot 3")
    bus, dev = Bus(os.environ["TERSE_BUS_SOCKET"]), Vector()
    while True:
        m = bus.recv()
        if m is None:
            return  # the core has gone
        kind = m[0]
        if kind == 0x01:  # HELLO
            if m[2] != 3 or m[3] != 0:
                bus.send(b"\x7fvector.py implements device 3 version 0")
                return
            name = b"vector (Python)"
            bus.send(bytes([0x81, 0, 3, 0, 0]) + struct.pack("<H", len(name)) + name +
                     struct.pack("<HHBB", 0, 0, 0, len(TABLE)) + b"".join(TABLE))
        elif kind == 0x02:  # RESET
            dev = Vector()
            bus.send(b"\x82")
        elif kind == 0x10:  # CALL
            bus.send(call(dev, m))
        elif kind == 0x11:  # BATCH: carried out in order, no replies
            count, at = struct.unpack_from("<I", m, 4)[0], 8
            for _ in range(count):
                n = struct.unpack_from("<I", m, at)[0]
                call(dev, m[at + 4:at + 4 + n])
                at += 4 + n
        elif kind == 0x30:  # BEGIN: no inputs
            bus.send(b"\xb0\0\0\0" + bytes(16))
        elif kind == 0x31:  # END
            pass
        elif kind == 0x40:  # STATE
            s = dev.state()
            bus.send(b"\xc0\0\0\0" + struct.pack("<I", len(s)) + s)
        elif kind == 0x41:  # RESTORE
            n = struct.unpack_from("<I", m, 4)[0]
            ok = n == len(m) - 8 and dev.restore(m[8:])
            bus.send(bytes([0xC1, 1 if ok else 0, 0, 0]))
        elif kind == 0x50:  # PRESENT: no layers; key 1, figures as outlines
            outlined = 1 if dev.style == 1 or dev.outlines else 0
            bus.send(b"\xd0\0\0\0" + struct.pack("<IBxxxI", 1, 1, outlined))
        elif kind == 0x60:  # SHUTDOWN
            return
        else:
            bus.send(b"\x7fvector.py: unknown message %02Xh" % kind)
            return


if __name__ == "__main__":
    main()
