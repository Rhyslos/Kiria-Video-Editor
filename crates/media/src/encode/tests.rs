//! What is written is what video sites and players ask for: a key frame
//! every two seconds or sooner, each starting a closed group, H.264 in its
//! High profile, BT.709 tags.

use super::*;
use crate::ffi::Input;

const SIZE: (u32, u32) = (320, 180);

/// Three and a half seconds at 30 fps of a slowly changing picture with a
/// cut to a quite different one at frame 40 (where an encoder may put a
/// key frame of its own).
fn write(path: &Path, container: Container, codec: VideoCodec) {
    let video = VideoSettings {
        codec,
        width: SIZE.0,
        height: SIZE.1,
        rate: (30, 1),
        bit_rate: 1_000_000,
        software: true,
    };
    let mut writer = Writer::create(path, container, Some(&video), None).unwrap();
    let (w, h) = (SIZE.0 as usize, SIZE.1 as usize);
    for frame in 0..105usize {
        let scene = if frame < 40 { 0 } else { 1 };
        let mut nv12: Vec<u8> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                if scene == 0 {
                    (16 + (x + frame * 2) % 200) as u8
                } else {
                    (16 + ((x / 8 + y / 8) % 2) * 200 + frame % 10) as u8
                }
            })
            .collect();
        nv12.extend(std::iter::repeat_n(
            if scene == 0 { 100u8 } else { 170 },
            w * h / 2,
        ));
        writer.write_picture(&nv12).unwrap();
    }
    writer.finish().unwrap();
}

/// The NAL unit types in a packet, whether it starts with start codes or
/// with four-byte lengths (as MP4 and MKV store them).
fn nal_types(data: &[u8], codec: VideoCodec) -> Vec<u8> {
    let kind = |header: u8| match codec {
        VideoCodec::H264 => header & 0x1f,
        _ => (header >> 1) & 0x3f,
    };
    let mut types = Vec::new();
    if data.starts_with(&[0, 0, 1]) || data.starts_with(&[0, 0, 0, 1]) {
        let mut i = 0;
        while i + 3 < data.len() {
            if data[i..i + 3] == [0, 0, 1] {
                types.push(kind(data[i + 3]));
                i += 3;
            } else {
                i += 1;
            }
        }
    } else {
        let mut i = 0;
        while i + 4 < data.len() {
            let n = u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
            types.push(kind(data[i + 4]));
            i += 4 + n;
        }
    }
    types
}

/// Each picture packet's time (in frames at 30 fps), whether it is marked
/// as a key frame and its NAL unit types; and the stream's profile and
/// colour tags.
struct Read {
    packets: Vec<(i64, bool, Vec<u8>)>,
    profile: i32,
    tags: (
        ff::AVColorPrimaries,
        ff::AVColorTransferCharacteristic,
        ff::AVColorSpace,
        ff::AVColorRange,
    ),
}

fn read(path: &Path, codec: VideoCodec) -> Read {
    let input = Input::open(path).unwrap();
    let ctx = input.raw();
    let mut packets = Vec::new();
    // SAFETY: the context is open and owned by `input`, used on this thread
    // only; the stream index is checked against `nb_streams`; the packet is
    // allocated here, filled by av_read_frame, its data read only while it
    // holds it, unreferenced after each and freed at the end.
    unsafe {
        assert!((*ctx).nb_streams >= 1);
        let stream = &**(*ctx).streams;
        let par = &*stream.codecpar;
        let tb = stream.time_base;
        let tags = (
            par.color_primaries,
            par.color_trc,
            par.color_space,
            par.color_range,
        );
        let mut packet = ff::av_packet_alloc();
        assert!(!packet.is_null());
        while ff::av_read_frame(ctx, packet) >= 0 {
            let p = &*packet;
            if p.stream_index == 0 && !p.data.is_null() {
                let data = std::slice::from_raw_parts(p.data, p.size as usize);
                let frame = (p.pts as f64 * f64::from(tb.num) / f64::from(tb.den) * 30.0).round() as i64;
                packets.push((frame, p.flags & ff::AV_PKT_FLAG_KEY != 0, nal_types(data, codec)));
            }
            ff::av_packet_unref(packet);
        }
        ff::av_packet_free(&mut packet);
        Read {
            packets,
            profile: par.profile,
            tags,
        }
    }
}

#[test]
fn key_frames_come_every_two_seconds_and_start_closed_groups() {
    let dir = tempfile::tempdir().unwrap();
    for (container, codec) in [
        (Container::Mp4, VideoCodec::H264),
        (Container::Mkv, VideoCodec::H264),
        (Container::Mp4, VideoCodec::H265),
        (Container::Mkv, VideoCodec::H265),
    ] {
        let what = format!("{codec:?} in {container:?}");
        let path = dir.path().join(format!("gop.{}", container.extension()));
        write(&path, container, codec);
        let read = read(&path, codec);
        assert_eq!(read.packets.len(), 105, "{what}");
        let mut keys: Vec<i64> = read.packets.iter().filter(|p| p.1).map(|p| p.0).collect();
        keys.sort_unstable();
        assert_eq!(keys.first(), Some(&0), "{what}: {keys:?}");
        keys.push(105);
        assert!(
            keys.windows(2).all(|pair| pair[1] - pair[0] <= 60),
            "{what}: a key frame at least every two seconds: {keys:?}"
        );
        // Closed groups: every key frame is an instant decoder refresh
        // (H.264 type 5; H.265 types 19 and 20), and nothing is a clean
        // random access point (H.265 type 21) or a picture that leads one
        // and refers to the group before (types 8 and 9).
        let refresh: &[u8] = match codec {
            VideoCodec::H264 => &[5],
            _ => &[19, 20],
        };
        for (frame, key, types) in &read.packets {
            if *key {
                assert!(
                    types.iter().any(|t| refresh.contains(t)),
                    "{what}: the key frame at {frame} is {types:?}"
                );
            }
            if codec == VideoCodec::H265 {
                assert!(
                    !types.iter().any(|t| [8, 9, 21].contains(t)),
                    "{what}: an open group at {frame}: {types:?}"
                );
            }
        }
        if codec == VideoCodec::H264 {
            assert_eq!(read.profile, ff::AV_PROFILE_H264_HIGH, "{what}");
        }
        assert_eq!(
            read.tags,
            (
                ff::AVColorPrimaries::AVCOL_PRI_BT709,
                ff::AVColorTransferCharacteristic::AVCOL_TRC_BT709,
                ff::AVColorSpace::AVCOL_SPC_BT709,
                ff::AVColorRange::AVCOL_RANGE_MPEG
            ),
            "{what}"
        );
        std::fs::remove_file(&path).unwrap();
    }
}

fn encoder_at(place: Place) -> Encoder {
    Encoder {
        name: "test",
        place,
        needs_upload: false,
    }
}

const fn asks(high_profile: bool, closed_groups: bool) -> Asks {
    Asks {
        high_profile,
        closed_groups,
    }
}

#[test]
fn a_graphics_cards_encoder_is_asked_again_for_less_before_the_next_is_tried() {
    for place in [Place::GraphicsCard, Place::System] {
        // H.264: everything, then without the High profile but with closed
        // groups, then without either.
        assert_eq!(
            Asks::in_order(&encoder_at(place), VideoCodec::H264),
            [asks(true, true), asks(false, true), asks(false, false)],
            "{place:?}"
        );
        // No profile is asked of the others: closed groups, then without.
        for codec in [VideoCodec::H265, VideoCodec::Av1] {
            assert_eq!(
                Asks::in_order(&encoder_at(place), codec),
                [asks(false, true), asks(false, false)],
                "{place:?} {codec:?}"
            );
        }
    }
    // The processor's encoders take everything: asked once.
    assert_eq!(
        Asks::in_order(&encoder_at(Place::Processor), VideoCodec::H264),
        [asks(true, true)]
    );
    assert_eq!(
        Asks::in_order(&encoder_at(Place::Processor), VideoCodec::H265),
        [asks(false, true)]
    );
}

#[test]
fn what_was_left_out_is_said_in_plain_words() {
    let all = asks(true, true);
    assert!(all.dropped(all).is_empty());
    assert_eq!(
        asks(false, true).dropped(all),
        ["the High profile (its own profile is used)"]
    );
    assert_eq!(asks(false, false).dropped(all).len(), 2);
    assert!(asks(false, false).dropped(all)[1].starts_with("closed groups"));
    // Nothing is said of a profile that wasn't asked for.
    assert_eq!(asks(false, false).dropped(asks(false, true)).len(), 1);
}

/// A probe's refusal is logged as its reason only: the fix meant for a
/// failed export (room on the drive) would send someone the wrong way.
#[test]
fn a_probed_refusal_is_logged_without_the_export_fix() {
    let err = cannot("Opening the encoder h264_nvenc failed: Operation not permitted.")
        .fix("Check that the folder can be written to and has room, then export again.");
    let line = refusal(&err);
    assert!(line.contains("Operation not permitted"), "{line}");
    assert!(!line.contains("room"), "{line}");
}
