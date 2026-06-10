use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use miru_codec::{i420_to_rgb, DecodedFrame};

fn make_frame(w: u32, h: u32) -> DecodedFrame {
    let y_size = (w * h) as usize;
    let uv_size = ((w / 2) * (h / 2)) as usize;
    DecodedFrame {
        width: w,
        height: h,
        y_plane: vec![128u8; y_size],
        u_plane: vec![128u8; uv_size],
        v_plane: vec![128u8; uv_size],
        timestamp_ms: 0,
    }
}

fn bench_color_convert(c: &mut Criterion) {
    let mut group = c.benchmark_group("i420_to_rgb");
    for &(w, h, name) in &[
        (640, 480, "vga"),
        (1280, 720, "720p"),
        (1920, 1080, "1080p"),
        (3840, 2160, "4k"),
    ] {
        let frame = make_frame(w, h);
        group.bench_with_input(BenchmarkId::from_parameter(name), &frame, |b, f| {
            b.iter(|| {
                let _ = i420_to_rgb(black_box(f));
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_color_convert);
criterion_main!(benches);
