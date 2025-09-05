# Alters the git-upload-pack stdout to print progress messages and sleep so that
# timeout behaviour can be tested.
import time
import sys

buf = b""
while True:
    line = sys.stdin.buffer.read(1)
    if line is None or line == b"":
        break
    buf += line
    assert sys.stdout.buffer.write(line) == 1, repr(line)
    if buf.endswith(b"000dpackfile\n"):
        # Around the time when the packfile starts, print a progress message and
        # sleep a bit.
        sys.stdout.buffer.write(b"000e\x02sleep 2s\n")
        time.sleep(2)
        sys.stdout.buffer.write(b"000e\x02sleep 2s\n")
        time.sleep(2)
    sys.stdout.buffer.flush()
