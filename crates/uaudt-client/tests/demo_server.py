"""Demo OPC-UA server for uaudt-client integration tests and benchmarks.

Exposes, like a PLC does:
  * a binary type dictionary (crates/uaudt-core/tests/fixtures/recipe.bsd) under i=93,
  * its type description nodes, the Recipe data type and its "Default Binary" encoding,
  * variables holding arrays of Recipe ExtensionObjects:
      ns=<idx>;s=Recipes      (3 elements)
      ns=<idx>;s=BigRecipes   (1000 elements, 10 sub-items each)
      ns=<idx>;s=SingleRecipe (scalar)

Usage: python demo_server.py [port]   (requires `pip install asyncua`)
"""
import asyncio
import sys
from pathlib import Path

from asyncua import Server, ua

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "src"))
from uaudtcodec._encoder import StructuredTypeEncoder  # noqa: E402
from uaudtcodec._parser import StructuredTypeParser  # noqa: E402

XML = (ROOT / "crates/uaudt-core/tests/fixtures/recipe.bsd").read_bytes()
NS_URI = "urn:uaudtcodec:demo"


def recipe(i, sub_items):
    return {
        "Name": f"Recipe {i}", "Enabled": True, "Count": i, "Mode": "Auto",
        "Created": 133000000000000000,
        "SubItems": [{"Id": j, "Description": f"Sub item {j}", "Value": j * 0.5, "Active": j % 2 == 0}
                     for j in range(sub_items)],
        "Origin": {"X": 1.5, "Y": float(i)}, "Tags": ["a", "b"],
    }


async def main(port):
    server = Server()
    await server.init()
    server.set_endpoint(f"opc.tcp://127.0.0.1:{port}/")
    server.set_security_policy([ua.SecurityPolicyType.NoSecurity])
    idx = await server.register_namespace(NS_URI)

    # Type dictionary under OPCBinarySchema_TypeSystem (i=93)
    type_system = server.get_node(ua.NodeId(ua.ObjectIds.OPCBinarySchema_TypeSystem))
    dict_node = await type_system.add_variable(
        ua.NodeId("TypeDictionary", idx), ua.QualifiedName("DemoTypes", idx),
        ua.Variant(XML, ua.VariantType.ByteString), datatype=ua.NodeId(ua.ObjectIds.ByteString))

    enc = StructuredTypeEncoder(*(lambda p: (p.get_structured_types(), p.get_enumeration_types()))(
        StructuredTypeParser(XML)))
    structure = server.get_node(ua.NodeId(ua.ObjectIds.Structure))
    encodings = {}
    for n, name in enumerate(["Recipe", "SubItem", "Point"]):
        desc = await dict_node.add_variable(
            ua.NodeId(f"Desc.{name}", idx), ua.QualifiedName(name, idx), name)
        dtype = await structure.add_data_type(ua.NodeId(3000 + n, idx), ua.QualifiedName(name, idx))
        encoding = await server.nodes.objects.add_object(
            ua.NodeId(4000 + n, idx), ua.QualifiedName("Default Binary", 0))
        await dtype.add_reference(encoding.nodeid, ua.ObjectIds.HasEncoding, True, True)
        await encoding.add_reference(desc.nodeid, ua.ObjectIds.HasDescription, True, True)
        encodings[name] = (dtype.nodeid, encoding.nodeid)

    dtype, encoding = encodings["Recipe"]

    def ext(i, sub_items):
        return ua.ExtensionObject(TypeId=encoding, Body=enc.encode_by_name("Recipe", recipe(i, sub_items)))

    for name, values in [("Recipes", [ext(i, 3) for i in range(3)]),
                         ("BigRecipes", [ext(i, 10) for i in range(1000)])]:
        var = await server.nodes.objects.add_variable(
            ua.NodeId(name, idx), ua.QualifiedName(name, idx),
            ua.Variant(values, ua.VariantType.ExtensionObject), datatype=dtype)
        await var.write_value_rank(1)
        await var.set_writable()
    single = await server.nodes.objects.add_variable(
        ua.NodeId("SingleRecipe", idx), ua.QualifiedName("SingleRecipe", idx),
        ua.Variant(ext(0, 2), ua.VariantType.ExtensionObject), datatype=dtype)
    await single.set_writable()

    async with server:
        print(f"READY opc.tcp://127.0.0.1:{port}/ ns={idx}", flush=True)
        while True:
            await asyncio.sleep(3600)


if __name__ == "__main__":
    asyncio.run(main(int(sys.argv[1]) if len(sys.argv) > 1 else 48400))
