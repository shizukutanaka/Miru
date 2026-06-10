//! Crypto throughput benchmarks.
//! Run with: cargo bench -p miru-common

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use miru_common::crypto::SessionCipher;

fn bench_encrypt_small(c: &mut Criterion) {
    let cipher = SessionCipher::new([42u8; 32]);
    let data = vec![0xABu8; 256];
    let mut g = c.benchmark_group("cipher_encrypt");
    g.throughput(Throughput::Bytes(data.len() as u64));
    g.bench_function("256B", |b| {
        b.iter(|| black_box(cipher.encrypt(&data).unwrap()))
    });
    g.finish();
}

fn bench_encrypt_frame(c: &mut Criterion) {
    let cipher = SessionCipher::new([42u8; 32]);
    // Typical encoded video frame size ~30KB
    let data = vec![0xABu8; 30 * 1024];
    let mut g = c.benchmark_group("cipher_encrypt");
    g.throughput(Throughput::Bytes(data.len() as u64));
    g.bench_function("30KB_frame", |b| {
        b.iter(|| black_box(cipher.encrypt(&data).unwrap()))
    });
    g.finish();
}

fn bench_roundtrip(c: &mut Criterion) {
    let cipher = SessionCipher::new([42u8; 32]);
    let data = vec![0xABu8; 8 * 1024];
    c.bench_function("cipher_roundtrip_8KB", |b| {
        b.iter(|| {
            let ct = cipher.encrypt(&data).unwrap();
            black_box(cipher.decrypt(&ct).unwrap())
        })
    });
}

criterion_group!(benches, bench_encrypt_small, bench_encrypt_frame, bench_roundtrip);
criterion_main!(benches);
