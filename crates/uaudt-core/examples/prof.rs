//! Quick per-operation timings on the golden `Recipe` (used for profiling).
//! `cargo run --release --example prof [iterations] [encode|decode|all]`
use std::time::Instant;
use uaudt_core::*;
fn main() {
    let xml = include_bytes!("../tests/fixtures/recipe.bsd");
    let d = TypeDictionary::parse(xml).unwrap();
    let id = d.require_struct("Recipe").unwrap();
    let body = include_bytes!("../tests/fixtures/recipe.bin");
    let v = d.decode(id, body).unwrap();
    let n: u32 = std::env::args()
        .nth(1)
        .map_or(2_000_000, |a| a.parse().unwrap());
    let mode = std::env::args().nth(2).unwrap_or_else(|| "all".into());
    if mode != "decode" {
        let t = Instant::now();
        let mut buf = Vec::with_capacity(512);
        for _ in 0..n {
            buf.clear();
            d.encode_into(std::hint::black_box(&v), &mut buf).unwrap();
        }
        println!("encode_into (reused buffer): {:?}/op", t.elapsed() / n);
        let t = Instant::now();
        for _ in 0..n {
            std::hint::black_box(d.encode(std::hint::black_box(&v)).unwrap());
        }
        println!("encode: {:?}/op", t.elapsed() / n);
    }
    if mode != "encode" {
        let t = Instant::now();
        for _ in 0..n {
            std::hint::black_box(d.decode(id, std::hint::black_box(body)).unwrap());
        }
        println!("decode: {:?}/op", t.elapsed() / n);
    }
}
