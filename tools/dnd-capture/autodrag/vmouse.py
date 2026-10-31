# SPDX-License-Identifier: GPL-3.0-or-later
"""The uinput absolute pointer for the automated drag runner (P0a2).

Declares a uinput device with EV_ABS ABS_X/ABS_Y (range 0..65535) and
EV_KEY BTN_LEFT — nothing else, so it cannot type or scroll.  Every kernel
constant below is computed from /usr/include/linux/uinput.h,
/usr/include/linux/input-event-codes.h, /usr/include/linux/input.h and
/usr/include/asm-generic/ioctl.h with the defining header line cited in the
comment; none is written from memory.

libinput classification: see NOTES-autodrag.md ("How libinput sees this
device").  Short version: udev tags this capability set ID_INPUT_MOUSE via
the absolute-mouse branch (systemd src/udev/udev-builtin-input_id.c, the
"VMware's USB mouse" path), so libinput treats it as a pointer, not a
touchscreen — INPUT_PROP_DIRECT is deliberately never set.  Needs confirming
live (creating the real device is the live check's job).

Usage (real):
    python3 -I vmouse.py --square --dry-run     # print a pointer-only square

API (used by autodrag.py and the tests):
    vm = VMouse(width, height)     # output size in logical pixels
    vm.move_to(x, y); vm.press(); vm.release(); vm.close()
    with VMouse(width, height) as vm: ...   # always releases + destroys

Tests replace the uinput layer entirely (see test_vmouse.py); nothing in
this module is exercised against the real /dev/uinput by the agent.
"""

import fcntl
import os
import struct
import sys
import time

# --------------------------------------------------------------------------
# ioctl number construction — /usr/include/asm-generic/ioctl.h
#
#   :23 #define _IOC_NRBITS	8
#   :24 #define _IOC_TYPEBITS	8
#   :32 #define _IOC_SIZEBITS	14
#   :44 #define _IOC_NRSHIFT	0
#   :45 #define _IOC_TYPESHIFT	(_IOC_NRSHIFT+_IOC_NRBITS)
#   :46 #define _IOC_SIZESHIFT	(_IOC_TYPESHIFT+_IOC_TYPEBITS)
#   :47 #define _IOC_DIRSHIFT	(_IOC_SIZESHIFT+_IOC_SIZEBITS)
#   :58 #define _IOC_NONE	0U
#   :62 #define _IOC_WRITE	1U
#   :66 #define _IOC_READ	2U
#   :69 #define _IOC(dir,type,nr,size) ...
#
_IOC_NRBITS = 8
_IOC_TYPEBITS = 8
_IOC_SIZEBITS = 14
_IOC_NRSHIFT = 0
_IOC_TYPESHIFT = _IOC_NRSHIFT + _IOC_NRBITS
_IOC_SIZESHIFT = _IOC_TYPESHIFT + _IOC_TYPEBITS
_IOC_DIRSHIFT = _IOC_SIZESHIFT + _IOC_SIZEBITS
_IOC_NONE = 0
_IOC_WRITE = 1
_IOC_READ = 2


def _ioc(direction, type_, nr, size):
    return (direction << _IOC_DIRSHIFT) | (size << _IOC_SIZESHIFT) | \
           (ord(type_) << _IOC_TYPESHIFT) | (nr << _IOC_NRSHIFT)


def _io(type_, nr):
    return _ioc(_IOC_NONE, type_, nr, 0)


def _iow(type_, nr, size):
    return _ioc(_IOC_WRITE, type_, nr, size)


# --------------------------------------------------------------------------
# event codes — /usr/include/linux/input-event-codes.h
#
#   :39 #define EV_SYN			0x00
#   :40 #define EV_KEY			0x01
#   :42 #define EV_ABS			0x03
#  :357 #define BTN_LEFT		0x110
#  :871 #define ABS_X			0x00
#  :872 #define ABS_Y			0x01
#   :24 #define INPUT_PROP_DIRECT	0x01   <- deliberately NEVER set
#
EV_SYN = 0x00
EV_KEY = 0x01
EV_ABS = 0x03
BTN_LEFT = 0x110
ABS_X = 0x00
ABS_Y = 0x01

SYN_REPORT = 0  # the only SYN code used; input-event-codes.h:34 (#define SYN_REPORT 0)

# bus type for the device id — /usr/include/linux/input.h:257
#   #define BUS_VIRTUAL		0x06
BUS_VIRTUAL = 0x06

# --------------------------------------------------------------------------
# uinput ioctls and structs — /usr/include/linux/uinput.h
#
#   :63 #define UINPUT_IOCTL_BASE	'U'
#   :64 #define UI_DEV_CREATE		_IO(UINPUT_IOCTL_BASE, 1)
#   :65 #define UI_DEV_DESTROY		_IO(UINPUT_IOCTL_BASE, 2)
#  :100 #define UI_DEV_SETUP _IOW(UINPUT_IOCTL_BASE, 3, struct uinput_setup)
#  :135 #define UI_ABS_SETUP _IOW(UINPUT_IOCTL_BASE, 4, struct uinput_abs_setup)
#  :137 #define UI_SET_EVBIT		_IOW(UINPUT_IOCTL_BASE, 100, int)
#  :138 #define UI_SET_KEYBIT		_IOW(UINPUT_IOCTL_BASE, 101, int)
#  :140 #define UI_SET_ABSBIT		_IOW(UINPUT_IOCTL_BASE, 103, int)
#  :147 #define UI_SET_PROPBIT		_IOW(UINPUT_IOCTL_BASE, 110, int)  (never used)
#   :47 #define UINPUT_MAX_NAME_SIZE	80
#
UINPUT_IOCTL_BASE = "U"
UINPUT_MAX_NAME_SIZE = 80

# struct uinput_setup (uinput.h:67-71): struct input_id id; char
# name[UINPUT_MAX_NAME_SIZE]; __u32 ff_effects_max;  — input_id is 4×__u16
# (input.h:57-62), name is 80 bytes, ff_effects_max is __u32: 8 + 80 + 4 = 92.
UINPUT_SETUP_FMT = "=HHHH%dsI" % UINPUT_MAX_NAME_SIZE

# struct uinput_abs_setup (uinput.h:102-106): __u16 code; __u16 filler;
# struct input_absinfo absinfo;  — input_absinfo is 6×__s32 (input.h:91-98):
# 2 + 2 pad + 24 = 28.
UINPUT_ABS_SETUP_FMT = "=H2x6i"

# struct input_event (input.h:26-38): struct timeval time; __u16 type;
# __u16 code; __s32 value.  On this machine's ABI (x86_64, __BITS_PER_LONG
# != 32, no __USE_TIME_BITS64) timeval is two 8-byte fields: 8+8+2+2+4 = 24,
# no padding.  The "@" (native) prefix matches the C ABI the kernel
# interface uses on this platform; the assert below fails loudly anywhere
# the layout would differ.
INPUT_EVENT_FMT = "@llHHi"
INPUT_EVENT_SIZE = struct.calcsize(INPUT_EVENT_FMT)
assert INPUT_EVENT_SIZE == 24, "unexpected input_event ABI: %d" % INPUT_EVENT_SIZE

ABS_RANGE_MAX = 65535  # declared max for ABS_X/ABS_Y (the brief's choice)

UI_DEV_CREATE = _io(UINPUT_IOCTL_BASE, 1)
UI_DEV_DESTROY = _io(UINPUT_IOCTL_BASE, 2)
# the _IOW sizes are the struct sizes they carry, so they are computed from
# the struct formats declared above (uinput.h:100/:135 pass
# sizeof(struct uinput_setup) / sizeof(struct uinput_abs_setup))
UI_DEV_SETUP = _iow(UINPUT_IOCTL_BASE, 3, struct.calcsize(UINPUT_SETUP_FMT))
UI_ABS_SETUP = _iow(UINPUT_IOCTL_BASE, 4, struct.calcsize(UINPUT_ABS_SETUP_FMT))
UI_SET_EVBIT = _iow(UINPUT_IOCTL_BASE, 100, 4)
UI_SET_KEYBIT = _iow(UINPUT_IOCTL_BASE, 101, 4)
UI_SET_ABSBIT = _iow(UINPUT_IOCTL_BASE, 103, 4)


def logical_to_abs(px, extent):
    """Output logical pixel -> uinput ABS value.

    ABS 0..65535 spans the output's 0..extent logical pixels linearly, so
    pixel p maps to p/extent of the range.  The compositor applies the
    inverse (libinput exposes
    libinput_event_pointer_get_absolute_x_transformed(event, width), which
    scales the axis value into `width`).  Whether niri transforms against
    the exact output width and where pixel centers land is confirmed live
    (NOTES-autodrag.md, TODO calibration); this is the mapping the dry-run
    prints so calibration can check it.
    """
    if extent <= 0:
        raise ValueError("output extent must be positive")
    v = int(round(float(px) * ABS_RANGE_MAX / extent))
    return max(0, min(ABS_RANGE_MAX, v))


def abs_to_logical(v, extent):
    """Inverse of logical_to_abs (for printing/diagnostics)."""
    return float(v) * extent / ABS_RANGE_MAX


def ui_get_sysname(length):
    """ioctl number for UI_GET_SYSNAME(len) — /usr/include/linux/uinput.h:161

        #define UI_GET_SYSNAME(len)	_IOC(_IOC_READ, UINPUT_IOCTL_BASE, 44, len)

    computed with the _ioc helper above, never written by hand."""
    return _ioc(_IOC_READ, UINPUT_IOCTL_BASE, 44, length)


class UinputDevice(object):
    """The real /dev/uinput layer.  Never touched by the agent or by the
    test suite against the real device: every external effect is injectable
    (opener/ioctl_fn/write_fn/closer for the kernel interface, sleep/clock
    for the waits, listdir + sysfs_root for the sysfs check), which is what
    the tests use (P0a2-r1 correction 3).

    Readiness sequence after UI_DEV_CREATE (libinput/niri add the device
    asynchronously through udev — without this, the first frames can land
    on Kyle's real pointer):
      1. read the sysfs device name with UI_GET_SYSNAME(64);
      2. poll /sys/devices/virtual/input/<name>/ for an `event*` child
         (timeout settle_timeout_s, then RuntimeError);
      3. sleep device_settle_ms;
    and before UI_DEV_DESTROY: sleep pre_destroy_ms so the last release
    has definitely landed."""

    def __init__(self, path="/dev/uinput", name="gimbal-autodrag pointer",
                 opener=None, ioctl_fn=None, write_fn=None, closer=None,
                 sleep=time.sleep, clock=time.monotonic, listdir=None,
                 sysfs_root="/sys/devices/virtual/input",
                 device_settle_ms=500, pre_destroy_ms=100,
                 settle_timeout_s=3.0, poll_s=0.05):
        self.path = path
        self.name = name
        self._opener = opener if opener is not None else (
            lambda p: os.open(p, os.O_WRONLY | os.O_CLOEXEC))
        self._ioctl = ioctl_fn if ioctl_fn is not None else fcntl.ioctl
        self._write = write_fn if write_fn is not None else os.write
        self._close = closer if closer is not None else os.close
        self._sleep = sleep
        self._clock = clock
        self._listdir = listdir if listdir is not None else os.listdir
        self.sysfs_root = sysfs_root
        self.pre_destroy_ms = pre_destroy_ms
        self.sysname = None
        self.fd = self._opener(path)
        try:
            for code in (EV_SYN, EV_KEY, EV_ABS):
                self._ioctl(self.fd, UI_SET_EVBIT, code)  # value, not a pointer: uinput reads the int from arg
            self._ioctl(self.fd, UI_SET_KEYBIT, BTN_LEFT)
            # No UI_SET_PROPBIT call at all: INPUT_PROP_DIRECT (the
            # touchscreen marker, input-event-codes.h:24) is never set.
            self._ioctl(self.fd, UI_SET_ABSBIT, ABS_X)
            self._ioctl(self.fd, UI_SET_ABSBIT, ABS_Y)
            for axis in (ABS_X, ABS_Y):
                absinfo = (0, 0, ABS_RANGE_MAX, 0, 0, 0)  # value,min,max,fuzz,flat,resolution
                self._ioctl(self.fd, UI_ABS_SETUP,
                            struct.pack(UINPUT_ABS_SETUP_FMT, axis, *absinfo))
            setup = struct.pack(
                UINPUT_SETUP_FMT,
                BUS_VIRTUAL, 0x1234, 0x5678, 1,  # bustype, vendor, product, version
                name.encode()[:UINPUT_MAX_NAME_SIZE - 1].ljust(UINPUT_MAX_NAME_SIZE, b"\0"),
                0)  # ff_effects_max: no force feedback
            self._ioctl(self.fd, UI_DEV_SETUP, setup)
            self._ioctl(self.fd, UI_DEV_CREATE, 0)
            # readiness (see class docstring); on timeout the fd is closed
            # and RuntimeError raised — run_real records an abort
            self.sysname = self._read_sysname()
            self._wait_for_event_node(self.sysname, settle_timeout_s, poll_s)
            self._sleep(device_settle_ms / 1000.0)
        except Exception:
            if self.fd is not None:
                self._close(self.fd)
                self.fd = None
            raise

    def _read_sysname(self):
        """The uinput sysfs device name (e.g. "input25") via UI_GET_SYSNAME;
        the kernel writes it into the ioctl buffer."""
        buf = bytearray(64)
        self._ioctl(self.fd, ui_get_sysname(len(buf)), buf)
        return bytes(buf).split(b"\0")[0].decode()

    def _wait_for_event_node(self, sysname, timeout_s, poll_s):
        deadline = self._clock() + timeout_s
        base = "%s/%s" % (self.sysfs_root.rstrip("/"), sysname)
        while True:
            try:
                if any(f.startswith("event") for f in self._listdir(base)):
                    return
            except OSError:
                pass  # node not there (yet)
            if self._clock() >= deadline:
                raise RuntimeError(
                    "uinput device %s got no event* node within %.0f s "
                    "(udev/libinput did not bind it); aborting rather than "
                    "clicking with an unbound device" % (base, timeout_s))
            self._sleep(poll_s)

    def emit(self, ev_type, code, value):
        self._write(self.fd, struct.pack(INPUT_EVENT_FMT, 0, 0, ev_type, code, value))

    def destroy(self):
        if self.fd is None:
            return
        try:
            self._sleep(self.pre_destroy_ms / 1000.0)
            self._ioctl(self.fd, UI_DEV_DESTROY, 0)
        finally:
            self._close(self.fd)
            self.fd = None


class FakeUinputDevice(object):
    """Test double for UinputDevice: records every ioctl and event, opens
    nothing.  Interface-compatible with UinputDevice."""

    def __init__(self, path="/dev/uinput-fake", name="fake"):
        self.path = path
        self.name = name
        self.fd = -1
        self.ioctls = []
        self.events = []  # (type, code, value)

    def emit(self, ev_type, code, value):
        self.events.append((ev_type, code, value))

    def destroy(self):
        self.fd = None

    def event_names(self):
        """Human-readable event list for assertions (decoded per type, so
        code 0 means SYN_REPORT for EV_SYN but ABS_X for EV_ABS)."""
        rev = {EV_SYN: "EV_SYN", EV_KEY: "EV_KEY", EV_ABS: "EV_ABS"}
        rcode = {EV_SYN: {SYN_REPORT: "SYN_REPORT"},
                 EV_KEY: {BTN_LEFT: "BTN_LEFT"},
                 EV_ABS: {ABS_X: "ABS_X", ABS_Y: "ABS_Y"}}
        return [(rev.get(t, t), rcode.get(t, {}).get(c, c), v)
                for (t, c, v) in self.events]


class VMouse(object):
    """Absolute pointer in output logical pixels.  Context manager: on any
    exit (exception included) the button is released and the device is
    destroyed — the release-then-destroy order the runner's safety rules
    require (P0a2 hard limit 5)."""

    def __init__(self, width, height, device=None):
        if width <= 0 or height <= 0:
            raise ValueError("output size must be positive")
        self.width = width
        self.height = height
        self._device = device if device is not None else UinputDevice()
        self.pressed = False

    # -- frame primitives --------------------------------------------------
    def _frame(self, events):
        """Send one event frame; EV_SYN SYN_REPORT closes every frame so the
        compositor sees each change atomically."""
        for (t, c, v) in events:
            self._device.emit(t, c, v)
        self._device.emit(EV_SYN, SYN_REPORT, 0)

    def move_to(self, x, y):
        """Move the pointer to logical output pixel (x, y)."""
        x = max(0, min(self.width - 1, int(round(x))))
        y = max(0, min(self.height - 1, int(round(y))))
        self._frame([
            (EV_ABS, ABS_X, logical_to_abs(x, self.width)),
            (EV_ABS, ABS_Y, logical_to_abs(y, self.height)),
        ])

    def press(self):
        if not self.pressed:
            self._frame([(EV_KEY, BTN_LEFT, 1)])
            self.pressed = True

    def release(self):
        """Release the button.  Safe to call repeatedly (the finally paths
        and the context manager may both release)."""
        if self.pressed:
            self._frame([(EV_KEY, BTN_LEFT, 0)])
            self.pressed = False

    # -- lifecycle ----------------------------------------------------------
    def close(self):
        """Release (if needed), then destroy the uinput device."""
        self.release()
        self._device.destroy()

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        try:
            self.release()
        finally:
            self._device.destroy()
        return False


# --------------------------------------------------------------------------
# CLI: python3 -I vmouse.py --square --dry-run
#
SQUARE_SIDE_PX = 200
SQUARE_DWELL_MS = 400  # pause at each corner (P0a2-r1 correction 3)


def _trace_square(vm, side, out, sleep=None, dwell_ms=0):
    """Move the pointer around a square centered on the screen center, no
    buttons.  Emits through `vm` so a fake backend can be checked in tests;
    returns the visited points.  With sleep+dwell_ms (the real run: 400 ms,
    P0a2-r1 correction 3) it pauses at each corner so the cursor position
    can be seen; the dry-run passes sleep=None and never waits."""
    cx, cy = vm.width // 2, vm.height // 2
    half = side // 2
    corners = [(cx - half, cy - half), (cx + half, cy - half),
               (cx + half, cy + half), (cx - half, cy + half),
               (cx - half, cy - half)]
    for (x, y) in corners:
        out.write("move_to (%d, %d) -> ABS (%d, %d)%s\n" % (
            x, y, logical_to_abs(x, vm.width), logical_to_abs(y, vm.height),
            "  [dwell %d ms]" % dwell_ms if sleep is not None and dwell_ms else ""))
        vm.move_to(x, y)
        if sleep is not None and dwell_ms:
            sleep(dwell_ms / 1000.0)
    return corners


def main(argv):
    dry_run = False
    square = False
    width = height = None
    side = SQUARE_SIDE_PX
    dwell_ms = SQUARE_DWELL_MS
    i = 0
    while i < len(argv):
        a = argv[i]
        if a == "--square":
            square = True
        elif a == "--dry-run":
            dry_run = True
        elif a == "--width":
            i += 1; width = int(argv[i])
        elif a == "--height":
            i += 1; height = int(argv[i])
        elif a == "--side":
            i += 1; side = int(argv[i])
        elif a == "--dwell-ms":
            i += 1; dwell_ms = int(argv[i])
        else:
            sys.stderr.write("usage: vmouse.py [--square] [--dry-run] "
                             "[--width W] [--height H] [--side S] "
                             "[--dwell-ms MS]\n")
            return 2
        i += 1
    if not square:
        sys.stderr.write("nothing to do: pass --square\n")
        return 2

    if width is None or height is None:
        # read-only niri query for the enabled output's logical size; falls
        # back to the size observed on this machine on 2026-10-30
        try:
            sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
            import niri as niri_mod
            outs = niri_mod.Niri().outputs()
            enabled = [o for o in outs.values() if o.get("logical")]
            width, height = enabled[0]["logical"]["width"], enabled[0]["logical"]["height"]
        except Exception as e:
            width, height = 2560, 1440
            sys.stderr.write("note: niri read failed (%s); using %dx%d\n" % (e, width, height))

    out = sys.stdout
    out.write("output: %dx%d logical px, ABS range 0..%d, mapping abs = px*%d/extent\n"
              % (width, height, ABS_RANGE_MAX, ABS_RANGE_MAX))
    out.write("device would declare: EV_ABS ABS_X/ABS_Y 0..%d, EV_KEY BTN_LEFT, EV_SYN "
              "(no props, no keys)\n" % ABS_RANGE_MAX)
    if dry_run:
        dev = FakeUinputDevice()
        vm = VMouse(width, height, device=dev)
        _trace_square(vm, side, out, sleep=None, dwell_ms=dwell_ms)
        out.write("no button events: %d frames, all EV_KEY counts: %d\n" % (
            sum(1 for e in dev.events if e[0] == EV_SYN),
            sum(1 for e in dev.events if e[0] == EV_KEY)))
        return 0

    try:
        vm = VMouse(width, height)  # waits for the event node + settle (r1 c3)
    except RuntimeError as e:
        sys.stderr.write("vmouse: %s\n" % e)
        return 1
    try:
        _trace_square(vm, side, out, sleep=time.sleep, dwell_ms=dwell_ms)
    finally:
        vm.close()
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
