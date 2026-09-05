//! Criterion benchmark: a full `compile()` of a generated 100- and 1000-file corpus.

use criterion::{criterion_group, criterion_main, Criterion};
use tempfile::tempdir;
use wiki::generator::generate_corpus;
use wiki::{compile, CompileOptions};

fn bench_compile(c: &mut Criterion) {
    for &n in &[100usize, 1000] {
        let dir = tempdir().unwrap();
        let input = dir.path().join("raw");
        generate_corpus(&input, n, 42).unwrap();
        c.bench_function(&format!("compile_{n}"), |b| {
            b.iter(|| {
                let out = tempdir().unwrap();
                compile(&input, &out.path().join("o"), &CompileOptions::default()).unwrap();
            });
        });
    }
}

criterion_group!(benches, bench_compile);
criterion_main!(benches);
