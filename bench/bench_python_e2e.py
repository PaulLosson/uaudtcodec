"""End-to-end timings of the Python UdtHandler against crates/uaudt-client/tests/demo_server.py
(same scenarios as crates/uaudt-client/examples/bench_e2e.rs)."""
import statistics, sys, time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))
import asyncua.sync  # noqa: E402
from uaudtcodec import UdtHandler  # noqa: E402

url = sys.argv[1] if len(sys.argv) > 1 else "opc.tcp://127.0.0.1:48400/"
ns = sys.argv[2] if len(sys.argv) > 2 else "2"
c = asyncua.sync.Client(url)
c.connect()
N = 10


def report(name, samples):
    print(f"{name:34} median {statistics.median(samples) * 1e3:10.3f}ms  min {min(samples) * 1e3:10.3f}ms")


def timed(fn, n=N):
    out = []
    for i in range(n):
        s = time.perf_counter(); fn(i); out.append(time.perf_counter() - s)
    return out


report("init (browse i=93 + open)", timed(lambda i: UdtHandler(c, f"ns={ns};s=BigRecipes")))
u = UdtHandler(c, f"ns={ns};s=BigRecipes")
report("read + decode 1000 recipes", timed(lambda i: u.read()))
report("decode only 1000 recipes", timed(lambda i: u._encoder.decode_list(u._type_name, u._raw_values)))
report("patch 1 element (full array write)",
       timed(lambda i: u.patch(500, {"Name": f"Patched {i}", "SubItems[3].Value": 1.0})))
s = UdtHandler(c, f"ns={ns};s=Recipes")
report("patch 1 element (3-element array)", timed(lambda i: s.patch(1, {"Count": i}), N * 5))
c.disconnect()
