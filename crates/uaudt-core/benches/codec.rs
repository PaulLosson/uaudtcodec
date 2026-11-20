//! Benchmarks on a realistic workload: 1 000 `Recipe` bodies, 10 sub-items each.
//! The same workload is measured for the Python implementation by `bench/bench_python.py`.

use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use uaudt_core::{StructValue, TypeDictionary, Value};

const XML: &[u8] = include_bytes!("../tests/fixtures/recipe.bsd");
pub const ELEMENTS: usize = 1_000;
pub const SUB_ITEMS: usize = 10;

fn build(dict: &TypeDictionary, i: usize) -> StructValue {
    let id = dict.require_struct("Recipe").unwrap();
    let sub = dict.require_struct("SubItem").unwrap();
    let mut v = dict.default_struct(id).unwrap();
    dict.set_str(&mut v, "Name", format!("Recipe {i}").into())
        .unwrap();
    dict.set_str(&mut v, "Enabled", true.into()).unwrap();
    dict.set_str(&mut v, "Count", (i as i32).into()).unwrap();
    dict.set_str(&mut v, "Mode", "Auto".into()).unwrap();
    dict.set_str(&mut v, "Created", Value::DateTime(133_000_000_000_000_000))
        .unwrap();
    let items = (0..SUB_ITEMS)
        .map(|j| {
            let mut s = dict.default_struct(sub).unwrap();
            dict.set_str(&mut s, "Id", (j as i32).into()).unwrap();
            dict.set_str(&mut s, "Description", format!("Sub item {j}").into())
                .unwrap();
            dict.set_str(&mut s, "Value", (j as f32 * 0.5).into())
                .unwrap();
            dict.set_str(&mut s, "Active", (j % 2 == 0).into()).unwrap();
            Value::Struct(s)
        })
        .collect();
    dict.set_str(&mut v, "SubItems", Value::Array(items))
        .unwrap();
    dict.set_str(&mut v, "Origin.X", 1.5f32.into()).unwrap();
    dict.set_str(&mut v, "Tags", Value::Array(vec!["a".into(), "b".into()]))
        .unwrap();
    v
}

fn benches(c: &mut Criterion) {
    let dict = TypeDictionary::parse(XML).unwrap();
    let id = dict.require_struct("Recipe").unwrap();
    let values: Vec<StructValue> = (0..ELEMENTS).map(|i| build(&dict, i)).collect();
    let bodies: Vec<Vec<u8>> = values.iter().map(|v| dict.encode(v).unwrap()).collect();
    let total: usize = bodies.iter().map(Vec::len).sum();

    c.bench_function("parse_dictionary", |b| {
        b.iter(|| TypeDictionary::parse(black_box(XML)).unwrap())
    });

    let mut g = c.benchmark_group("1000_recipes");
    g.throughput(Throughput::Bytes(total as u64));
    g.bench_function("decode", |b| {
        b.iter(|| {
            bodies
                .iter()
                .map(|body| dict.decode(id, black_box(body)).unwrap())
                .collect::<Vec<_>>()
        })
    });
    g.bench_function("encode", |b| {
        b.iter(|| {
            values
                .iter()
                .map(|v| dict.encode(black_box(v)).unwrap())
                .collect::<Vec<_>>()
        })
    });
    g.finish();

    let name = dict.compile_path(id, "Name").unwrap();
    let desc = dict.compile_path(id, "SubItems[7].Description").unwrap();
    let body = &bodies[0];
    let mut g = c.benchmark_group("single_element");
    g.bench_function("patch_bytes_2_fields", |b| {
        b.iter(|| {
            dict.patch(
                black_box(body),
                &[(&name, "Renamed".into()), (&desc, "Changed".into())],
            )
            .unwrap()
        })
    });
    g.bench_function("patch_decode_set_encode_2_fields", |b| {
        b.iter(|| {
            let mut v = dict.decode(id, black_box(body)).unwrap();
            dict.set(&mut v, &name, "Renamed".into()).unwrap();
            dict.set(&mut v, &desc, "Changed".into()).unwrap();
            dict.encode(&v).unwrap()
        })
    });
    g.bench_function("read_path", |b| {
        b.iter(|| dict.read_path(black_box(body), &desc).unwrap())
    });
    g.finish();
}

criterion_group!(codec, benches);
criterion_main!(codec);
