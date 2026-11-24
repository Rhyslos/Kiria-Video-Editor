# render-cache spike

Compares ways of keeping rendered frames on disk for the render cache (`docs/research/render-cache.md`).

```sh
FFMPEG=path/to/ffmpeg ./make-frames.sh frames.rgba 30 2560x1440
cargo run --release -- frames.rgba 2560 1440 work/
FFMPEG=path/to/ffmpeg ./video.sh frames.rgba 2560 1440 work/
```

`src/main.rs` times raw, LZ4, Zstandard and JPEG (the `image` crate) per frame: compressing, writing with a sync, and reading back and decompressing. `video.sh` times H.264 (every frame a key frame, or one every 8), FFV1 and JPEG through FFmpeg, decoded on one thread.
