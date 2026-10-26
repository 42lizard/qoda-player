fn main() {
    // Guard the ALSA entry points used by libmpv in this executable only.
    // No LD_PRELOAD helper, process re-exec or system configuration is involved.
    for symbol in [
        "snd_pcm_open",
        "snd_pcm_hw_params",
        "snd_pcm_writei",
        "snd_pcm_writen",
        "snd_pcm_mmap_begin",
        "snd_pcm_close",
        "snd_pcm_mmap_commit",
        "snd_mixer_selem_set_playback_volume",
        "snd_mixer_selem_set_playback_volume_all",
        "snd_mixer_selem_set_playback_dB",
        "snd_mixer_selem_set_playback_dB_all",
        "snd_mixer_selem_set_playback_switch",
        "snd_mixer_selem_set_playback_switch_all",
    ] {
        println!("cargo:rustc-link-arg=-Wl,--export-dynamic-symbol={symbol}");
    }
}
