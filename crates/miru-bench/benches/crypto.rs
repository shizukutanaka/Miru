use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use miru_common::crypto::SessionCipher;

fn bench_cipher(c: &mut Criterion) {
    let cipher = SessionCipher::new([42u8; 32]);

    let mut group = c.benchmark_group("encrypt");
    for &size in &[1024usize, 16 * 1024, 256 * 1024, 1024 * 1024] {
        let data = vec![0u8; size];
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &data, |b, d| {
            b.iter(|| {
                let _ = cipher.encrypt(black_box(d));
            });
        });
    }
    group.finish();

    let mut group = c.benchmark_group("decrypt");
    for &size in &[1024usize, 16 * 1024, 256 * 1024, 1024 * 1024] {
        let plain = vec![0u8; size];
        let ct = cipher.encrypt(&plain).unwrap();
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &ct, |b, c2| {
            b.iter(|| {
                let _ = cipher.decrypt(black_box(c2));
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_cipher);
criterion_main!(benches);
