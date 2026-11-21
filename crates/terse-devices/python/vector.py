#!/usr/bin/env python3
"""Terse device 3, the 2D vector display (SPEC.md §vector, version 0), and, if
the core asks for it too, device 9, the 3D vector display (spec/vector3d.md §vector3d,
version 0), as a device driver of the device bus (DEVICES.md, protocol
version 1), in Python 3 with the standard library only. It draws nothing
(no layers); it keeps each machine's device state (STATE.md 1.7, 1.11) and
publishes the properties `vector.outlines` (figures and filled shapes drawn
as outlines: fill mode 1 or the phosphor style), `vector3.list` (device
9's list) and `vector.list` (device 3's state section, for the system ink
and the phosphor style's beam, which the core draws) (DEVICES.md 7.2).
Socket transport only.

It answers for exactly the slots the core asks for in HELLO (3, or 3 and
9), so start it for slot 3 alone (`terse-bus --driver 3=...`) or for both
(`--driver 3,9=...`).

    TERSE_BUS_SOCKET=<path> vector.py
    vector.py --version        # name, version and build identifier

Its HELLO reply ends with its identity (DEVICES.md 1.1): the version and a
build identifier, `b2:` and the BLAKE2b hash (8 bytes) of this file.
"""
import hashlib
import os
import socket
import struct
import sys

MAX_LIST = 16384
SLOT2D, SLOT3D = 3, 9
VERSION = 1
ERR_UNKNOWN = 0xFFFFFFFF
OUTLINES, LIST3, LIST2 = b"vector.outlines", b"vector3.list", b"vector.list"
PROGRAM_VERSION = "0.1.0"


def build_id():
    """`b2:` and BLAKE2b (digest 8 bytes) of this script, or `unknown`."""
    try:
        with open(os.path.abspath(__file__), "rb") as f:
            return "b2:" + hashlib.blake2b(f.read(), digest_size=8).hexdigest()
    except OSError:
        return "unknown"


BUILD = build_id()


class Instance:
    """One machine's displays: device 3's state (SPEC §vector.determinism: style,
    persistence, fill mode, list), there for the shared style also when
    only device 9 is listed, and device 9's list if it is listed."""

    def __init__(self, lists2, lists3):
        self.lists2 = lists2
        self.list3 = b"" if lists3 else None
        self.reset()

    def reset(self):
        self.style, self.persistence, self.outlines, self.list = 0, 160, 0, b""
        if self.list3 is not None:
            self.list3 = b""

    def call(self, slot, fn, a, data):
        """Carries out a call; returns (a0, charge on top of the 4)."""
        if fn == 0 and (slot == SLOT2D or self.list3 is not None):  # submit (the core checked length and place)
            if not 1 <= a[1] <= MAX_LIST or len(data) != a[1]:
                return 1, 0
            if slot == SLOT2D:
                self.list = bytes(data)
            else:
                self.list3 = bytes(data)
            return 0, (len(data) + 3) // 4
        if fn == 1 and (slot == SLOT2D or self.list3 is not None):  # clear
            if slot == SLOT2D:
                self.list = b""
            else:
                self.list3 = b""
            return 0, 0
        # The style and persistence are device 3's, also through device 9
        # (SPEC §vector3d.calls); the fill mode is device 3's alone.
        if fn == 2:
            if a[0] > 1:
                return 1, 0
            self.style = a[0]
            return 0, 0
        if fn == 3:
            if a[0] > 255:
                return 1, 0
            self.persistence = a[0]
            return 0, 0
        if fn == 4 and slot == SLOT2D:
            if a[0] > 1:
                return 1, 0
            self.outlines = a[0]
            return 0, 0
        return ERR_UNKNOWN, 0

    def state(self, slot):  # after the present flag
        if slot == SLOT2D:
            return bytes([self.style, self.persistence, self.outlines]) + struct.pack("<I", len(self.list)) + self.list
        lst = self.list3 or b""
        return struct.pack("<I", len(lst)) + lst

    def restore(self, slot, s):
        """Takes the slot's section from the start of s; its length, or None
        if it is not a valid section (then nothing changed)."""
        if slot == SLOT2D:
            if len(s) < 7 or s[0] > 1 or s[2] > 1:
                return None
            m = struct.unpack_from("<I", s, 3)[0]
            if m > MAX_LIST or len(s) < 7 + m:
                return None
            # Without device 3 the fill mode and the 2D list cannot be set.
            if not self.lists2 and (s[2] or m):
                return None
            self.style, self.persistence, self.outlines, self.list = s[0], s[1], s[2], bytes(s[7:7 + m])
            return 7 + m
        if self.list3 is None or len(s) < 4:
            return None
        m = struct.unpack_from("<I", s, 0)[0]
        if m > MAX_LIST or len(s) < 4 + m:
            return None
        self.list3 = bytes(s[4:4 + m])
        return 4 + m

    def get(self, name):
        """A property it publishes, or None."""
        if name == OUTLINES:
            return bytes([1 if self.style == 1 or self.outlines else 0])
        if name == LIST3:
            return self.list3 or b""
        if name == LIST2:
            return self.state(SLOT2D)
        return None


def entry(fn, rule, value=0, submit=False):
    """A 64-byte call table entry (DEVICES.md 5): deferred, so batchable,
    with a result rule; submit's buffer is a0, a1 x 1, 1-16,384 bytes, read,
    RAM only, both failures 1, charged 1 per 4 bytes. An entry without a
    buffer keeps the buffer results FFFF_FFFEh, as the Rust tables do."""
    if submit:
        buf = (0x06, 0, 1, 1, 1, MAX_LIST, 0x01, 0xFF, 0, 1, 1)
    else:
        buf = (0x02, 0, 0xFF, 0, 0, 0, 0, 0xFF, 0, 0xFFFFFFFE, 0xFFFFFFFE)
    flags, addr, length, scale, least, greatest, access, checked, cmax, lres, pres = buf
    return struct.pack("<BBBBIIIBBHIII", fn, flags, addr, length, scale, least, greatest,
                       access, checked, 0, cmax, lres, pres) + \
        struct.pack("<BBHIII", rule, 0, 0, value, 0, 0) + \
        struct.pack("<BBHIII", 1 if submit else 0, 0xFF, 0, 0, 0, 0)


TABLES = {
    SLOT2D: [entry(0, 1, submit=True), entry(1, 1), entry(2, 3, 1), entry(3, 3, 255), entry(4, 3, 1)],
    SLOT3D: [entry(0, 1, submit=True), entry(1, 1), entry(2, 3, 1), entry(3, 3, 255)],
}


def string(b):
    return struct.pack("<H", len(b)) + b


def blob(b):
    return struct.pack("<I", len(b)) + b


def names(items):
    return bytes([len(items)]) + b"".join(string(n) for n in items)


class Bus:
    def __init__(self, path):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(path)

    def recv(self):
        """The next message: (instance, slot, body), or None at the end."""
        head = self.exact(4)
        m = self.exact(struct.unpack("<I", head)[0]) if head else None
        if m is None:
            return None
        if len(m) < 9:
            raise SystemExit("vector.py: a message without an address or a type")
        return struct.unpack_from("<I", m, 0)[0], m[4], m[8:]

    def exact(self, n):
        out = bytearray()
        while len(out) < n:
            chunk = self.sock.recv(n - len(out))
            if not chunk:
                return None
            out += chunk
        return bytes(out)

    def send(self, instance, slot, body):
        """A message with the address of its request (DEVICES.md 1)."""
        self.sock.sendall(struct.pack("<IIB3x", 8 + len(body), instance, slot) + body)


def read_str(m, at):
    n = struct.unpack_from("<H", m, at)[0]
    return m[at + 2:at + 2 + n], at + 2 + n


def call(inst, slot, m):
    fn = m[1]
    a = struct.unpack_from("<6I", m, 4)
    n = struct.unpack_from("<I", m, 36)[0]
    a0, charge = inst.call(slot, fn, a, m[40:40 + n])
    return struct.pack("<BBHIIII", 0x90, 0, 0, a0, 0, charge, 0)


def hello(bus, m):
    """Answers HELLO for exactly the slots asked; the slots it serves."""
    if m[1] != VERSION:
        bus.send(0, 0, b"\x7fvector.py speaks protocol version 1")
        return None
    s = m[8]
    asked = list(m[9:9 + s])
    if not asked or any(x not in TABLES for x in asked):
        bus.send(0, 0, b"\x7fvector.py serves slots 3 and 9, not %r" % (asked,))
        return None
    name = b"vector (Python)"
    out = bytes([0x81, VERSION, 0, 0]) + string(name) + struct.pack("<HHB", 0, 0, len(asked))
    for slot in asked:  # each: version 0, no layers, the call table
        out += bytes([slot, 0, 0, len(TABLES[slot])]) + b"".join(TABLES[slot])
    out += names([OUTLINES, LIST3, LIST2]) + names([]) + names([])
    out += string(PROGRAM_VERSION.encode()) + string(BUILD.encode())  # the identity
    bus.send(0, 0, out)
    return asked


def open_instance(m, served):
    """OPEN: (Instance, None) if taken, else (None, the reason)."""
    _, at = read_str(m, 44)  # the memory file: empty, on the socket
    listed = {m[at + 1 + 2 * i]: m[at + 2 + 2 * i] for i in range(m[at])}
    mine = {s: v for s, v in listed.items() if s in served}
    if any(v != 0 for v in mine.values()):
        return None, b"vector.py implements version 0"
    if SLOT3D in mine and SLOT2D not in served:
        return None, b"device 9 needs this driver to serve device 3 too (SPEC \xc2\xa7vector3d.calls)"
    return Instance(SLOT2D in mine, SLOT3D in mine), None


def main():
    if sys.argv[1:] == ["--version"]:
        print("vector.py %s %s" % (PROGRAM_VERSION, BUILD))
        return
    bus = Bus(os.environ["TERSE_BUS_SOCKET"])
    served = None
    insts = {}
    while True:
        msg = bus.recv()
        if msg is None:
            return  # the core has gone
        to, slot, m = msg
        kind = m[0]
        if served is None:
            if kind != 0x01:
                bus.send(to, slot, b"\x7fvector.py: expected HELLO")
                return
            served = hello(bus, m)
            if served is None:
                return
            continue
        if kind not in (0x03, 0x04, 0x11, 0x60, 0x7F) and to not in insts:
            bus.send(to, slot, b"\x7fvector.py: no instance %d" % to)
            return
        inst = insts.get(to)
        if kind == 0x03:  # OPEN
            inst, why = open_instance(m, served)
            insts.pop(to, None)
            if inst:
                insts[to] = inst
                bus.send(to, slot, bytes([0x83, 1, 0, 0]) + string(b""))
            else:
                bus.send(to, slot, bytes([0x83, 0, 0, 0]) + string(why))
        elif kind == 0x04:  # CLOSE
            insts.pop(to, None)
        elif kind == 0x02:  # RESET
            inst.reset()
            bus.send(to, slot, b"\x82")
        elif kind == 0x10:  # CALL
            bus.send(to, slot, call(inst, slot, m))
        elif kind == 0x11:  # BATCH: each a blob (address, body), carried out in order, no replies
            count, at = struct.unpack_from("<I", m, 4)[0], 8
            for _ in range(count):
                n = struct.unpack_from("<I", m, at)[0]
                c_to, c_slot = struct.unpack_from("<IB", m, at + 4)
                if c_to not in insts:
                    bus.send(c_to, c_slot, b"\x7fvector.py: a batched call for no instance %d" % c_to)
                    return
                call(insts[c_to], c_slot, m[at + 12:at + 4 + n])
                at += 4 + n
        elif kind == 0x30:  # BEGIN: no inputs
            bus.send(to, slot, b"\xb0\0\0\0" + bytes(16))
        elif kind == 0x31:  # END
            pass
        elif kind == 0x40:  # STATE
            bus.send(to, slot, b"\xc0\0\0\0" + blob(inst.state(slot)))
        elif kind == 0x41:  # RESTORE: the section from the start of the rest of a serialization
            n = struct.unpack_from("<I", m, 4)[0]
            taken = inst.restore(slot, m[8:8 + n]) if n == len(m) - 8 else None
            bus.send(to, slot, struct.pack("<BBHI", 0xC1, taken is not None, 0, taken or 0))
        elif kind == 0x42:  # GET
            count, at, out = m[2], 3, []
            for _ in range(count):
                name, at = read_str(m, at)
                value = inst.get(name)
                if value is not None:
                    out.append(string(name) + blob(value))
            bus.send(to, slot, bytes([0xC2, len(out)]) + b"".join(out))
        elif kind == 0x43:  # SET: it subscribes to nothing
            pass
        elif kind == 0x44:  # SERVICE: it gives none
            bus.send(to, slot, b"\xc4\0" + string(b"vector.py gives no services") + blob(b""))
        elif kind == 0x50:  # PRESENT: it draws nothing, no layers
            bus.send(to, slot, b"\xd0\0\0\0")
        elif kind == 0x60:  # SHUTDOWN
            return
        elif kind == 0x7F:  # ERROR from the core: it is about to close
            return
        else:
            bus.send(to, slot, b"\x7fvector.py: unknown message %02Xh" % kind)
            return


if __name__ == "__main__":
    main()
