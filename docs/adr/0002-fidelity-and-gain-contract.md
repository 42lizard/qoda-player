# Explicit fidelity path and precision-aware ReplayGain

Qoda defaults to desktop PipeWire playback and offers an explicitly selected direct ALSA fidelity path that refuses unsupported conversion, fixes app volume at unity, and allows selected ReplayGain processing. Because measured mpv integer-output gain uses coarse coefficient steps, ReplayGain uses the qualified high-precision per-file filter path with native gain disabled; gain-off playback bypasses that filter. Configuration intent and verified output are separate states: the contract names the ALSA PCM handoff as its first qualification boundary and makes no unmeasured DAC claim.

The canonical defaults, fallback rules, gain provenance, gapless conditions, and qualification gate are recorded in [Choose verifiable fidelity and ReplayGain guarantees](https://github.com/42lizard/qoda-player/issues/8). No real-device qualification has yet been completed.
