# Background removal

How the editor takes the background out from behind a person on a facecam (build plan 9.4). The project plan describes it as a matte: the person is kept, everything behind them becomes see-through, with clean hair edges and no flicker. It is computed once in the background and played back in real time from then on.

Preliminary: the candidates and their licences are settled here, but the comparison on real facecam footage is still to do (section 3).

## 1. Problem

**What it must do.** A facecam is a webcam or camera recording of one person, usually from the chest up, sitting in front of a room, a wall or a green screen. The matte must:

- keep hair, headsets, glasses and hands, with soft edges where hair is fine;
- stay steady from frame to frame: no shimmer at the edges, no holes that come and go;
- cope with the usual facecam: dim rooms, a monitor's glow on the face, a busy shelf or a gaming chair behind, fast head turns.

**Numbers.**

| What | Target |
| --- | --- |
| Computing the matte | faster than real time on a mid-range graphics card at 1080p (a ten-minute facecam in under ten minutes); usable on the processor alone, however slowly |
| Playing afterwards | real time at the timeline's size: the matte is read from the cache, never computed while playing |
| Size | the model shipped or fetched once, ideally under 50 MB |
| Gate | the difficult-clips set (hair against busy backgrounds, fast motion, low light): no visible flicker and clean hair edges at full resolution |

**Licence.** The app is GPL-3.0-or-later. The model's code and weights, and the runtime that runs it, must be GPL-compatible. Weights for non-commercial use only are not compatible.

## 2. Approaches

Four kinds of model are in use, and one candidate for each was looked at.

| Model | What it is | Licence | Size | Speed (published) | Fit |
| --- | --- | --- | --- | --- | --- |
| Robust Video Matting (RVM), MobileNetV3 | A matting network built for video: it carries a memory from frame to frame (recurrent), so the matte is steady by design. Gives an alpha matte and the person's colour. Works at full resolution by running small and refining. | Code GPL-3.0; ONNX exports provided (opset 12, FP32 and FP16), tested by its authors on ONNX Runtime with the processor and CUDA | small (the MobileNetV3 variant is the one recommended) | 1080p: 134 fps on an RTX 2060 Super (FP16), 104 fps on a GTX 1080 Ti (FP32); tensor throughput only | Best fit: made for exactly this, steady over time, fast, GPL-compatible. The weights' own licence is not stated separately and must be confirmed with the authors before shipping. |
| MODNet | A portrait matting network for single pictures and webcams, no memory between frames. | Apache-2.0 on the ONNX copy looked at; the original release must be checked, as copies differ | small | real time on webcams (published) | Second choice: good edges, but flicker between frames has to be smoothed afterwards. |
| BiRefNet (matting variant) | A large, very accurate network for cutting out objects in single pictures at 1024 × 1024. | MIT | about 900 MB (FP32 ONNX) | slow: one picture at a time, a large model | Not for video: too large and too slow, no memory between frames. Possibly for a single "cut out this still" later. |
| MediaPipe Selfie Segmentation | A tiny network that tells person from background, made for video calls. | Apache-2.0 | 447 KB | very fast, at 256 × 144 or 256 × 256 | A fallback where nothing else runs: a coarse mask, no hair detail. |

**The runtime.** ONNX Runtime (MIT) runs all four. It is reached from Rust through the `ort` crate (MIT or Apache-2.0), with DirectML on Windows for any graphics card, CUDA or ROCm where they are installed, and the processor otherwise. The runtime is a separate library, as FFmpeg is: it is loaded when background removal is first used, not linked into the editor, so the editor stays small and starts as fast as it does now.

**After the model.** None of the models alone reaches the gate. Two steps follow it in every approach:

- *Edge refinement at full resolution:* the model runs small, and a guided filter on the full-resolution picture brings the matte's edge back to the hair.
- *Steadiness over time:* a little smoothing of the matte along time where the picture hasn't moved (RVM needs least of this), with a strength slider.

## 3. Prototypes (to do)

Each candidate goes in `spikes/background-removal/`, run through `ort` on the same clips:

1. RVM MobileNetV3 (FP16 on the graphics card, FP32 on the processor).
2. MODNet with the time smoothing of section 2.
3. MediaPipe Selfie Segmentation, as the floor.

Each is measured on the owner's facecam recordings plus the difficult-clips set:

- time per frame at 1080p, on the graphics card and on the processor;
- flicker: how much the matte changes between frames where the picture barely does;
- edge quality: compared by eye against a hand-made matte of a few frames.

## 4. Result

Not yet measured. Expected from section 2: RVM first, with MediaPipe as the fallback where ONNX Runtime can't use a graphics card. To decide after section 3:

- Confirm the RVM weights' licence with its authors. If it can't be confirmed, MODNet takes its place.
- Ship the model inside the installer, or fetch it on first use from the update host with a checked signature, as updates are.

## Sources

- [Robust Video Matting, its README and speed table](https://github.com/PeterL1n/RobustVideoMatting)
- [MODNet in ONNX, with its licence tag](https://huggingface.co/Xenova/modnet)
- [BiRefNet matting in ONNX: licence, size, input](https://huggingface.co/emrikol/birefnet-matting-onnx)
- [MediaPipe Selfie Segmentation: licence, size, input](https://aihub.qualcomm.com/mobile/models/mediapipe_selfie)
- [ort, the Rust bindings to ONNX Runtime](https://github.com/pykeio/ort)
