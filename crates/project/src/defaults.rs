//! Values left out of files when they are their default (format 18).
//!
//! A field marked `#[serde(default, skip_serializing_if =
//! "crate::defaults::is_default")]` is written only when it differs from
//! its type's [`Default`], and read back as that default when missing, so
//! an untouched clip is written as little more than its place and its file.
//!
//! A default is therefore part of the file format: changing what a
//! written type's `Default` is would quietly change every file that left
//! the value out. It needs a new format version whose migration writes the
//! old value in (`tests::defaults_are_frozen` fails first, pointing here).

/// Whether `value` is its type's default, and so left out of files.
pub(crate) fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

#[cfg(test)]
mod tests {
    use crate::look::Look;
    use crate::model::{ClipKind, FrameBlend, MediaInfo};
    use crate::sound::{ClipLines, Sound};
    use crate::time::Speed;
    use crate::track_sound::{MasterSound, TrackSound};

    /// The defaults files written in format 18 and later rely on. Changing
    /// one changes what every file that left it out means: give the change
    /// a new format version whose migration writes the old value in, then
    /// update the text here.
    #[test]
    fn defaults_are_frozen() {
        assert_eq!(
            ron::to_string(&Look::default()).unwrap(),
            "(x:0,y:0,scale:1000000,turn:0,crop_left:0,crop_top:0,crop_right:0,crop_bottom:0,opacity:1000000,exposure:0,contrast:0,saturation:0,temperature:0,tint:0)"
        );
        assert_eq!(ron::to_string(&Speed::default()).unwrap(), "(num:1,den:1)");
        assert_eq!(ron::to_string(&Sound::default()).unwrap(), SOUND);
        assert_eq!(
            ron::to_string(&ClipLines::default()).unwrap(),
            "(volume:[],pan:[])"
        );
        assert_eq!(ron::to_string(&ClipKind::default()).unwrap(), "File");
        assert_eq!(ron::to_string(&FrameBlend::default()).unwrap(), "Off");
        assert_eq!(ron::to_string(&TrackSound::default()).unwrap(), TRACK_SOUND);
        assert_eq!(ron::to_string(&MasterSound::default()).unwrap(), MASTER_SOUND);
        assert_eq!(ron::to_string(&MediaInfo::default()).unwrap(), MEDIA_INFO);
    }

    const SOUND: &str = "(gain_mdb:0,pan:0,fade_in:(0,1),fade_out:(0,1))";
    const TRACK_SOUND: &str = concat!(
        "(role:Other,pan:0,high_pass:(on:false,freq_hz:80),eq:(on:false,bands:((on:true,s",
        "hape:LowShelf,freq_hz:100,q_milli:1000,gain_mdb:0),(on:true,shape:Peak,freq_hz:2",
        "50,q_milli:1000,gain_mdb:0),(on:true,shape:Peak,freq_hz:1000,q_milli:1000,gain_m",
        "db:0),(on:true,shape:Peak,freq_hz:3000,q_milli:1000,gain_mdb:0),(on:true,shape:P",
        "eak,freq_hz:6000,q_milli:1000,gain_mdb:0),(on:true,shape:HighShelf,freq_hz:10000",
        ",q_milli:1000,gain_mdb:0))),compressor:(on:false,threshold_mdb:-24000,ratio_mill",
        "i:3000,attack_us:5000,release_us:120000,knee_mdb:6000,makeup_mdb:4000),cleanup:(",
        "on:false,threshold_mdb:6000,reduction_mdb:-18000,release_us:120000,profile:None)",
        ",ducking:(on:false,threshold_mdb:-40000,hold_us:300000,down_us:40000,up_us:40000",
        "0,depth_mdb:-12000),loudness:(on:false,target_mlufs:-18000,max_boost_mdb:12000,m",
        "ax_cut_mdb:12000),volume_line:[],pan_line:[])",
    );
    const MASTER_SOUND: &str = concat!(
        "(compressor:(on:false,threshold_mdb:-24000,ratio_milli:3000,attack_us:5000,relea",
        "se_us:120000,knee_mdb:6000,makeup_mdb:4000),limiter:(on:false,ceiling_mdb:-1000)",
        ")",
    );
    const MEDIA_INFO: &str = "(container:\"\",video:None,audio:[])";
}
