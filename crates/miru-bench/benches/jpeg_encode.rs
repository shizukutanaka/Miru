use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use miru_codec::{i420_to_jpeg, DecodedFrame};

fn make_frame(w: u32, h: u32) -> DecodedFrame {
    DecodedFrame {
        width: w,
        height: h,
        y_plane: vec![128u8; (w * h) as usize],
        u_plane: vec![128u8; ((w / 2) * (h / 2)) as usize],
        v_plane: vec![128u8; ((w / 2) * (h / 2)) as usize],
        timestamp_ms: 0,
    }
}

fn bench_jpeg(c: &mut Criterion) {
    let mut group = c.benchmark_group("jpeg_encode");
    for &(w, h, name) in &[(1280, 720, "720p"), (1920, 1080, "1080p")] {
        for &quality in &[50u8, 70, 90] {
            let frame = make_frame(w, h);
            group.bench_with_input(
                BenchmarkId::new(format!("{name}_q{quality}"), "default"),
                &frame,
                |b, f| {
                    b.iter(|| {
                        let _ = i420_to_jpeg(black_box(f), quality);
                    });
                },
            );
        }
    }
    group.finish();
}

criterion_group!(benches, bench_jpeg);
criterion_main!(benches);
