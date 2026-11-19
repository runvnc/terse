#!/usr/bin/env python3
"""A fake core host for testing device drivers of the Terse device bus
(DEVICES.md), stdlib only.

It plays the core's side: listens on a Unix socket in a temporary
directory, starts a driver, sends HELLO, checks calls against the
driver's call table as DEVICES.md 5 says (and batches the batchable ones,
section 6), and asks for STATE, RESTORE, BEGIN, PRESENT and so on.

    fake_core.py terminal <driver command> [--shm] [--oracle <term-oracle>] [--fuzz N]
    fake_core.py vector <driver command>

The terminal tests compare STATE bytes with expectations written here and,
with --oracle, run random call sequences through both the driver and a
small Rust program built on crates/terse-core's Terminal (it reads ops on
stdin: fn u8, a0 a1 a2 u32, budget i64, length u32, bytes; and prints
"r0 r1 charge hexwrites" per op and then "STATE hex"), comparing every
result, charge, write and the final state bit for bit.
"""
import atexit
import ctypes
import os
import platform
import random
import shlex
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import time
import mmap

ROM_BASE, RAM_BASE, RAM_FILE_OFF = 0x10000, 0x100000, 131072
ERR_BUFFER, ERR_UNKNOWN = 0xFFFFFFFE, 0xFFFFFFFF
AREA = 4 << 20


def u32(b, at):
    return struct.unpack_from("<I", b, at)[0]


def frame(body):
    return struct.pack("<I", len(body)) + body


class Entry:
    FMT = "<BBBBIIIBBHIIIBBHIIIBBHIII"

    def __init__(self, raw):
        (self.fn, self.flags, self.addr_arg, self.len_arg, self.scale, self.least,
         self.greatest, self.access, self.checked, _, self.checked_max, self.len_result,
         self.place_result, self.rule, self.rule_arg, _, self.value, self.value2,
         self.charge, self.charge_flags, self.charge_arg, self.per_byte, self.low, self.high,
         self.units) = struct.unpack(self.FMT, raw)


class Core:
    def __init__(self, cmd, slot, rom=b"", ram_len=65536, shm=False, batch=True):
        self.dir = tempfile.mkdtemp(prefix="fake-core-")
        atexit.register(shutil.rmtree, self.dir, True)
        self.slot, self.rom, self.ram_len, self.shm, self.batching = slot, rom, ram_len, shm, batch
        self.mem_path = os.path.join(self.dir, "memory")
        with open(self.mem_path, "wb") as f:
            f.truncate(RAM_FILE_OFF + ram_len)
            f.write(rom)
        fd = os.open(self.mem_path, os.O_RDWR)
        self.mem = mmap.mmap(fd, RAM_FILE_OFF + ram_len)
        os.close(fd)
        self.bus_path = os.path.join(self.dir, "bus")
        if shm:
            with open(self.bus_path, "wb") as f:
                f.truncate(4096 + 2 * AREA)
            fd = os.open(self.bus_path, os.O_RDWR)
            self.bus = mmap.mmap(fd, 4096 + 2 * AREA)
            os.close(fd)
            self.bus[0:24] = b"TBUS" + struct.pack("<IIIII", 0, 4096, AREA, 4096 + AREA, AREA)
            self.libc = ctypes.CDLL(None, use_errno=True)
            self.sys_futex = {"x86_64": 202, "aarch64": 98}[platform.machine()]
            self.futex_base = ctypes.addressof(ctypes.c_char.from_buffer(self.bus))
            # The control words, read and written whole (never torn).
            self.words = {o: ctypes.c_uint32.from_buffer(self.bus, o) for o in (64, 68, 72, 128, 132, 136)}
        path = os.path.join(self.dir, "bus.sock")
        lsock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        lsock.bind(path)
        lsock.listen(1)
        lsock.settimeout(10)
        env = dict(os.environ, TERSE_BUS_SOCKET=path, TERSE_BUS_SLOT=str(slot))
        if shm:
            env["TERSE_C_SHM"] = "1"
        self.proc = subprocess.Popen(cmd, env=env)
        atexit.register(lambda p=self.proc: p.poll() is None and p.kill())  # never leave one running
        self.sock, _ = lsock.accept()
        lsock.close()
        self.sock.settimeout(10)
        self.on_shm = False
        self.pending = []  # no-reply messages waiting for the next packet (shm)
        self.batch = []
        self.hello()

    # -- transport
    def recv_exact(self, n):
        out = b""
        while len(out) < n:
            chunk = self.sock.recv(n - len(out))
            if not chunk:
                raise EOFError("driver closed the socket")
            out += chunk
        return out

    def send(self, body, reply=True):
        """Sends one message; returns the reply's body if one is expected."""
        if not self.on_shm:
            self.sock.sendall(frame(body))
            return self.recv_socket() if reply else None
        self.pending.append(frame(body))
        if not reply and body[0] != 0x60:
            return None  # held for the next packet: one packet in flight
        packet = b"".join(self.pending)
        self.pending = []
        self.bus[4096:4096 + len(packet)] = packet
        w = self.words
        w[68].value = len(packet)
        rep_seq = w[128].value
        w[64].value = (w[64].value + 1) & 0xFFFFFFFF  # one aligned 32-bit store
        if w[72].value == 1:
            self.libc.syscall(self.sys_futex, ctypes.c_void_p(self.futex_base + 64), 1, 0x7FFFFFFF, None, None, 0)
        if not reply:
            return None
        deadline = time.monotonic() + 10
        while w[128].value == rep_seq:
            if time.monotonic() > deadline or self.proc.poll() is not None:
                raise EOFError("no reply on shared memory")
        n = w[132].value
        msg = bytes(self.bus[4096 + AREA:4096 + AREA + n])
        assert u32(msg, 0) == n - 4, "reply packet is not one message"
        return self.check_error(msg[4:])

    def recv_socket(self):
        n = u32(self.recv_exact(4), 0)
        return self.check_error(self.recv_exact(n))

    @staticmethod
    def check_error(body):
        if body[0] == 0x7F:
            raise RuntimeError("driver ERROR: " + body[1:].decode("utf-8", "replace"))
        return body

    def request(self, body, reply=True):
        """Any request but a batchable call: the batch goes first."""
        if self.batch:
            msg = struct.pack("<BxxxI", 0x11, len(self.batch)) + b"".join(frame(c) for c in self.batch)
            self.batch = []
            self.send(msg, reply=False)
        return self.send(body, reply)

    # -- handshake
    def hello(self):
        def s(text):
            b = text.encode()
            return struct.pack("<H", len(b)) + b
        flags = 1 if self.shm else 0
        body = struct.pack("<BBBBI", 1, 0, self.slot, 0, flags) + bytes(32)
        body += struct.pack("<II", len(self.rom), self.ram_len)
        body += s(self.mem_path if self.shm else "") + s(self.bus_path if self.shm else "") + s(self.dir)
        body += self.rom
        self.sock.sendall(frame(body))
        r = self.recv_socket()
        assert r[0] == 0x81 and r[1] == 0 and r[2] == self.slot and r[3] == 0, r[:4]
        transport = r[4]
        assert transport == (1 if self.shm else 0), "transport %d" % transport
        n = struct.unpack_from("<H", r, 5)[0]
        self.name = r[7:7 + n].decode()
        at = 7 + n
        self.native = struct.unpack_from("<HH", r, at)
        k = r[at + 4]
        self.layers = [tuple(r[at + 5 + 2 * i:at + 7 + 2 * i]) for i in range(k)]
        at += 5 + 2 * k
        m = r[at]
        at += 1
        assert len(r) == at + 64 * m, "call table length"
        self.table = {}
        for i in range(m):
            e = Entry(r[at + 64 * i:at + 64 * (i + 1)])
            self.table[e.fn] = e
        self.on_shm = transport == 1

    # -- memory
    def mem_off(self, addr, length, rom_ok):
        if length == 0:
            return 0
        if RAM_BASE <= addr and addr + length <= RAM_BASE + self.ram_len:
            return RAM_FILE_OFF + addr - RAM_BASE
        if rom_ok and ROM_BASE <= addr and addr + length <= ROM_BASE + len(self.rom):
            return addr - ROM_BASE
        return None

    def poke(self, addr, data):
        off = self.mem_off(addr, len(data), False)
        self.mem[off:off + len(data)] = data

    def peek(self, addr, n):
        off = self.mem_off(addr, n, True)
        return bytes(self.mem[off:off + n])

    # -- calls (DEVICES.md 5, 6)
    def call(self, fn, args, budget=125000):
        """Returns (a0, a1 or None, the driver's charge: on top of the call's
        base, 4 or, for an answered call, 64, which the core adds)."""
        args = list(args) + [0] * (6 - len(args))
        e = self.table.get(fn)
        if e is None:
            return ERR_UNKNOWN, None, 0
        buf = b""
        if e.flags & 4:
            length = e.scale if e.len_arg == 0xFF else args[e.len_arg] * e.scale
            if not e.least <= length <= e.greatest:
                return e.len_result, None, 0
            addr = args[e.addr_arg]
            off = self.mem_off(addr, length, bool(e.access & 4) and not e.access & 2)
            if off is None:
                return e.place_result, None, 0
            if e.checked != 0xFF and args[e.checked] > e.checked_max:
                return e.place_result, None, 0
            if e.access & 1:
                buf = bytes(self.mem[off:off + length])
        if self.batching and e.flags & 2:
            if e.rule == 1:
                a0, a1 = e.value, e.value2
            elif e.rule == 2:
                a0, a1 = args[e.rule_arg], 0
            elif e.rule == 3:
                a0, a1 = (0 if args[e.rule_arg] <= e.value else 1), 0
            else:
                raise AssertionError("batchable call without a result rule")
            charge = e.charge
            if e.charge_flags & 1:
                charge += (len(buf) + 3) // 4
            charge += len(buf) * e.per_byte
            if e.charge_arg != 0xFF and e.low <= args[e.charge_arg] <= e.high:
                charge += e.units
            self.batch.append(self.call_msg(fn, args, budget, buf))
            return a0, (a1 if e.flags & 1 else None), charge
        if self.on_shm:
            buf = b""
        r = self.request(self.call_msg(fn, args, budget, buf))
        assert r[0] == 0x90
        flags, a0, a1, charge, w = r[1], u32(r, 4), u32(r, 8), u32(r, 12), u32(r, 16)
        at = 20
        for _ in range(w):
            addr, n = u32(r, at), u32(r, at + 4)
            wa = args[e.addr_arg]
            wl = args[e.len_arg] * e.scale
            assert e.access & 2 and wa <= addr and addr + n <= wa + wl, "write outside the write buffer"
            self.poke(addr, r[at + 8:at + 8 + n])
            at += 8 + n
        assert at == len(r)
        return a0, (a1 if flags & 1 else None), charge

    @staticmethod
    def call_msg(fn, args, budget, buf):
        return struct.pack("<BBH6IqI", 0x10, fn, 0, *args, budget, len(buf)) + buf

    # -- the rest
    def state(self):
        r = self.request(bytes([0x40]))
        assert r[0] == 0xC0 and len(r) == 8 + u32(r, 4)
        return r[8:]

    def restore(self, s):
        r = self.request(struct.pack("<BxxxI", 0x41, len(s)) + s)
        assert r[0] == 0xC1 and len(r) == 4
        return r[1]

    def reset(self):
        assert self.request(bytes([0x02])) == bytes([0x82])

    def begin(self, frame_no=0):
        r = self.request(struct.pack("<BBHIII", 0x30, 0, 0, frame_no, 0, 0))
        assert r[0] == 0xB0
        return r

    def end(self, frame_no=1):
        self.request(struct.pack("<BBHI", 0x31, 0, 0, frame_no), reply=False)

    def present(self, props=()):
        body = struct.pack("<BxxxHH4fI", 0x50, 640, 480, 0, 0, 640, 480, len(props))
        for slot, key, value in props:
            body += struct.pack("<BBHI", slot, key, 0, value)
        r = self.request(body)
        assert r[0] == 0xD0
        return r

    def shutdown(self):
        self.request(bytes([0x60]), reply=False)
        self.proc.wait(timeout=5)
        self.sock.close()
        assert self.proc.returncode == 0, "driver exit %d" % self.proc.returncode


# ------------------------------------------------------------- terminal

def term_reset_state():
    cells = struct.pack("<HBB", 0x20, 7, 0) * 2400
    base = [0x000000, 0xCD0000, 0x00CD00, 0xCDCD00, 0x0000EE, 0xCD00CD, 0x00CDCD, 0xE5E5E5,
            0x7F7F7F, 0xFF0000, 0x00FF00, 0xFFFF00, 0x5C5CFF, 0xFF00FF, 0x00FFFF, 0xFFFFFF]
    pal = b"".join(c.to_bytes(3, "big") for c in base)
    cube = [0, 0x5F, 0x87, 0xAF, 0xD7, 0xFF]
    pal += b"".join(bytes([cube[i // 36], cube[i // 6 % 6], cube[i % 6]]) for i in range(216))
    pal += b"".join(bytes([8 + 10 * i] * 3) for i in range(24))
    return bytearray(cells + bytes([0, 0, 7, 0, 1, 0]) + pal + bytes(4))


def chars(state, row, n):
    return [struct.unpack_from("<H", state, (row * 80 + c) * 4)[0] for c in range(n)]


RAM = RAM_BASE


def write(core, data, budget=125000, at=RAM):
    core.poke(at, data)
    return core.call(0, [at, len(data)], budget)


def terminal_tests(core):
    s = core.state()
    assert len(s) == 10378 and s == term_reset_state(), "reset state"
    assert core.call(9, [])[:2] == (80, 30)
    # Text, colours, cursor.
    core.call(4, [3, 4])
    assert write(core, b"Hi\tx") == (4, None, 8)
    s = core.state()
    assert chars(s, 0, 9)[:3] == [0x48, 0x69, 0x20] and chars(s, 0, 9)[8] == ord("x")
    assert s[2:4] == bytes([3, 4])
    assert core.call(3, []) == (9, 0, 0) and core.call(5, []) == (3, 4, 0)
    core.call(4, [0x100, 9])  # above 255: foreground kept
    assert core.call(5, []) == (3, 9, 0)
    # Wide characters: at column 79 the pair moves to the next row.
    core.call(2, [79, 0])
    write(core, "好".encode())
    s = core.state()
    assert chars(s, 0, 80)[79] == 0x20 and chars(s, 1, 2) == [0x597D, 0xFFFF]
    assert core.call(3, []) == (2, 1, 0)
    # Malformed UTF-8: maximal subparts; an incomplete sequence is kept.
    core.call(1, [1])
    write(core, b"\xC3A\xE0\x80\x80\xED\xA0\x80\xF0\x9F\x98")
    s = core.state()
    assert chars(s, 0, 8) == [0xFFFD, 0x41] + [0xFFFD] * 6, [hex(c) for c in chars(s, 0, 8)]
    assert s[10374:10378] == b"\x03\xF0\x9F\x98", s[10374:]
    write(core, b"\x80")  # completes U+1F600: above FFFFh, so FFFDh
    s = core.state()
    assert chars(s, 0, 9)[8] == 0xFFFD and s[10374:10378] == bytes(4)
    # write takes the whole buffer whatever the budget: 2 per byte, and
    # nothing for the scrolls it causes.
    core.call(2, [0, 29])
    assert write(core, b"\n\nq", 1) == (3, None, 6)
    assert write(core, b"xyz", -50) == (3, None, 6)
    assert write(core, b"", 100) == (0, None, 0)
    # Commands.
    assert core.call(1, [2]) == (0, None, 150)
    assert core.call(1, [0x77]) == (0, None, 0)
    assert core.call(1, [0x102]) == (0, None, 0)
    core.call(1, [6])
    assert core.state()[9604] == 0
    # cells: wrap and the copy charge; checked argument and length.
    core.poke(RAM + 0x100, b"A\x00\x01\x02B\x00\x03\x04")
    assert core.call(7, [2399, RAM + 0x100, 2]) == (2, None, 2)
    assert core.call(6, [2399, RAM + 0x200, 3]) == (3, None, 3)
    assert core.peek(RAM + 0x200, 8) == b"A\x00\x01\x02B\x00\x03\x04"
    assert core.call(6, [2400, RAM + 0x200, 1])[0] == ERR_BUFFER
    assert core.call(7, [0, RAM + 0x200, 2401])[0] == ERR_BUFFER
    assert core.call(6, [0, ROM_BASE, 1])[0] == ERR_BUFFER  # written: not ROM
    # Palette, wrapping 255 -> 0.
    core.poke(RAM + 0x300, bytes([1, 2, 3, 4, 5, 6]))
    assert core.call(8, [255, RAM + 0x300, 2]) == (2, None, 2)
    s = core.state()
    assert s[9606 + 765:9606 + 768] == bytes([1, 2, 3]) and s[9606:9609] == bytes([4, 5, 6])
    # STATE, RESTORE (good and bad), RESET.
    good = core.state()
    bad = bytearray(good)
    bad[9600] = 80
    assert core.restore(bytes(bad)) == 0 and core.state() == good
    bad = bytearray(good)
    bad[10374:10378] = b"\x01\x41\x00\x00"  # not a lead byte
    assert core.restore(bytes(bad)) == 0
    assert core.restore(good[:-1]) == 0 and core.state() == good
    core.reset()
    assert core.state() == term_reset_state()
    seq = bytearray(good)
    seq[10374:10378] = b"\x02\xE4\xBD\x00"
    assert core.restore(bytes(seq)) == 1 and core.state() == seq
    write(core, b"\xA0")  # completes U+4F60
    assert core.state()[10374] == 0
    # BEGIN, END, PRESENT.
    r = core.begin()
    assert r == bytes([0xB0, 0, 0, 0]) + bytes(16), r
    core.end()
    assert core.present() == bytes([0xD0, 0, 0, 0]) + bytes(4)
    print("terminal: fixed scenarios ok")


def gen_text(rng):
    pieces = ["a", "Hello, world", "\n", "\r", "\t", "\b", "\x0c", "\x07", "\x7f", "你", "好", "ｱ", "Ａ",
              "é", "😀", "￿", "﩮", "〿", "　", "★"]
    out = b""
    for _ in range(rng.randint(0, 40)):
        r = rng.random()
        if r < 0.6:
            out += rng.choice(pieces).encode()
        elif r < 0.8:
            out += bytes([rng.choice([0x80, 0xBF, 0xC0, 0xC2, 0xE0, 0xED, 0xEF, 0xF0, 0xF4, 0xF5, 0xFF,
                                      0xA0, 0x9F, 0x90, 0x8F])])
        else:
            out += rng.choice(pieces).encode()[:-1]
    return out


def fuzz(cmd, oracle, n, seed, shm):
    rng = random.Random(seed)
    rom = gen_text(rng) * 50 + b"x" * 100
    core = Core(cmd, 2, rom=rom, shm=shm)
    ops = []
    for i in range(n):
        k = rng.random()
        if k < 0.45:
            data = gen_text(rng)
            budget = rng.choice([125000, 1, 2, 5, 20, 0, -10, 151, 300])
            if rng.random() < 0.15 and len(rom) > 10:
                at = rng.randrange(len(rom) - 5)
                ln = rng.randrange(min(60, len(rom) - at))
                addr, data = ROM_BASE + at, rom[at:at + ln]
            else:
                addr = RAM + rng.randrange(0, 4000)
            ops.append((0, [addr, len(data), 0], budget, data))
        elif k < 0.55:
            ops.append((1, [rng.choice([1, 2, 3, 4, 5, 6, 0, 7, 0x101, 255])], 0, b""))
        elif k < 0.65:
            ops.append((2, [rng.choice([0, 5, 77, 78, 79, 80, 1000]), rng.choice([0, 3, 28, 29, 30, 99])], 0, b""))
        elif k < 0.7:
            ops.append((4, [rng.choice([1, 2, 300]), rng.choice([0, 5, 256])], 0, b""))
        elif k < 0.75:
            ops.append((rng.choice([3, 5, 9]), [], 0, b""))
        elif k < 0.85:
            cnt = rng.choice([0, 1, 2, 3, 80, 2400, 2401])
            first = rng.choice([0, 79, 1000, 2398, 2399, 2400])
            data = bytes(rng.choice([0x20, 0x00, 0xFF, 0x4F, 0x60, 7, 3]) for _ in range(4 * min(cnt, 2400)))
            addr = RAM + 0x8000
            ops.append((7, [first, addr, cnt], 0, data if cnt <= 2400 else b""))
        elif k < 0.9:
            cnt = rng.choice([0, 1, 5, 300, 2400])
            ops.append((6, [rng.choice([0, 1500, 2399, 2400]), RAM + 0xC000, cnt], 0, b""))
        else:
            cnt = rng.choice([0, 1, 2, 256, 257])
            data = bytes(rng.randrange(256) for _ in range(3 * min(cnt, 256)))
            addr = RAM + 0x7000
            ops.append((8, [rng.choice([0, 200, 255, 256]), addr, cnt], 0, data if cnt <= 256 else b""))
    stream = b""
    for fn, args, budget, data in ops:
        a = (args + [0, 0, 0])[:3]
        stream += struct.pack("<B3IqI", fn, *a, budget, len(data)) + data
    want = subprocess.run([oracle], input=stream, capture_output=True, check=True).stdout.decode().split("\n")
    for i, (fn, args, budget, data) in enumerate(ops):
        if data and args[fn != 0] >= RAM:  # the buffer, put in RAM just before the call
            core.poke(args[fn != 0], data)
        a0, a1, charge = core.call(fn, args, budget)
        w0, w1, wc, whex = (want[i].split(" ") + [""])[:4]
        got = (a0, charge) if a1 is None else (a0, a1, charge)
        exp = (int(w0), int(wc)) if a1 is None else (int(w0), int(w1), int(wc))
        assert got == exp, "op %d fn %d %r budget %d: got %r want %r" % (i, fn, args, budget, got, exp)
        if fn == 6 and a0 != ERR_BUFFER:
            assert core.peek(args[1], 4 * args[2]).hex() == whex, "op %d cells_read bytes" % i
    s = core.state()
    assert want[len(ops)].startswith("STATE ")
    assert s.hex() == want[len(ops)][6:], "final state differs (seed %d)" % seed
    core.shutdown()


# --------------------------------------------------------------- vector

def vector_tests(core):
    assert core.layers == [] and all(e.flags & 2 for e in core.table.values())
    assert core.state() == bytes([0, 160, 0]) + bytes(4)
    lst = bytes([1, 255, 0, 0, 255, 4, 16, 0, 32, 0, 5, 0, 1, 0, 2])
    core.poke(RAM, lst)
    assert core.call(0, [RAM, len(lst)]) == (0, None, 4)
    assert core.call(0, [RAM, 0]) == (1, None, 0)
    assert core.call(0, [RAM, 16385]) == (1, None, 0)
    assert core.call(0, [ROM_BASE, 1]) == (1, None, 0)
    assert core.call(2, [2]) == (1, None, 0) and core.call(3, [256]) == (1, None, 0)
    assert core.call(4, [1]) == (0, None, 0) and core.call(3, [7]) == (0, None, 0)
    assert core.state() == bytes([0, 7, 1]) + struct.pack("<I", len(lst)) + lst
    r = core.present()
    assert r == bytes([0xD0, 0, 0, 0]) + struct.pack("<IBxxxI", 1, 1, 1), r
    core.call(4, [0])
    core.call(2, [1])
    assert core.present()[-4:] == struct.pack("<I", 1)
    core.call(2, [0])
    assert core.present()[-4:] == struct.pack("<I", 0)
    core.call(1, [])
    good = core.state()
    assert good == bytes([0, 7, 0]) + bytes(4)
    assert core.restore(bytes([2, 0, 0, 0, 0, 0, 0])) == 0 and core.state() == good
    assert core.restore(bytes([1, 9, 1, 2, 0, 0, 0, 0])) == 0  # m says 2, one byte
    assert core.restore(bytes([1, 9, 1, 1, 0, 0, 0, 7])) == 1
    assert core.state() == bytes([1, 9, 1, 1, 0, 0, 0, 7])
    assert core.begin() == bytes([0xB0, 0, 0, 0]) + bytes(16)
    core.end()
    core.reset()
    assert core.state() == bytes([0, 160, 0]) + bytes(4)
    print("vector: fixed scenarios ok")


def main():
    args = sys.argv[1:]
    if len(args) < 2:
        sys.exit(__doc__)
    kind, cmd = args[0], shlex.split(args[1])
    shm = "--shm" in args
    oracle = args[args.index("--oracle") + 1] if "--oracle" in args else None
    nfuzz = int(args[args.index("--fuzz") + 1]) if "--fuzz" in args else 20
    if kind == "terminal":
        for batching in (True, False):
            core = Core(cmd, 2, rom=b"ROM text", shm=shm, batch=batching)
            terminal_tests(core)
            core.shutdown()
        if oracle:
            for seed in range(nfuzz):
                fuzz(cmd, oracle, 300, seed, shm)
            print("terminal: %d random sequences match the Rust reference" % nfuzz)
        # The driver exits when the socket closes, without SHUTDOWN.
        core = Core(cmd, 2, shm=shm)
        core.sock.close()
        core.proc.wait(timeout=5)
        print("terminal: exits on EOF")
    elif kind == "vector":
        core = Core(cmd, 3)
        vector_tests(core)
        core.shutdown()
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
