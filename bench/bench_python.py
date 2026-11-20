"""Same workload as crates/uaudt-core/benches/codec.rs, on the Python implementation."""
import sys, timeit
from pathlib import Path

root = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(root / "src"))
from uaudtcodec._parser import StructuredTypeParser
from uaudtcodec._encoder import StructuredTypeEncoder

xml = (root / "crates/uaudt-core/tests/fixtures/recipe.bsd").read_bytes()
p = StructuredTypeParser(xml)
enc = StructuredTypeEncoder(p.get_structured_types(), p.get_enumeration_types())
unp = enc._get_unpacker()

def build(i):
    return {"Name": f"Recipe {i}", "Enabled": True, "Count": i, "Mode": "Auto",
            "Created": 133000000000000000,
            "SubItems": [{"Id": j, "Description": f"Sub item {j}", "Value": j * 0.5, "Active": j % 2 == 0}
                         for j in range(10)],
            "Origin": {"X": 1.5, "Y": 0.0}, "Tags": ["a", "b"]}

bodies = [enc.encode_by_name("Recipe", build(i)) for i in range(1000)]
values = [enc.decode("Recipe", b) for b in bodies]

def bench(name, fn, n):
    t = min(timeit.repeat(fn, number=n, repeat=5)) / n
    print(f"{name:40s} {t * 1e6:12.2f} µs")

bench("parse_dictionary", lambda: StructuredTypeParser(xml), 200)
bench("1000_recipes/decode", lambda: [unp.unpack("Recipe", b)[0] for b in bodies], 5)
bench("1000_recipes/encode", lambda: [enc.encode(v) for v in values], 5)
bench("single_element/patch_2_fields",
      lambda: enc.patch("Recipe", bodies[0], {"Name": "Renamed", "SubItems[7].Description": "Changed"}), 2000)
