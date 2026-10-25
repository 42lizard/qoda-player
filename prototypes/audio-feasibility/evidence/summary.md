# Audio feasibility measurements

Player-side PCM file capture and timed null output only. No physical audio device, PipeWire monitor or microphone used.

Measured 2026-10-25. libmpv client API 0x20005.

| Case | PCM result | Max s32 sample error | Observed output rates | Run time |
| --- | --- | --- | --- | --- |
| wav-unity | EXACT | 0 | [44100] | 0.299s |
| flac-unity | EXACT | 0 | [44100] | 0.165s |
| flac-no-prefetch | EXACT | 0 | [44100] | 0.162s |
| bit-depth-16-24 | EXACT | 0 | [44100] | 0.167s |
| rate-strong | OBSERVATION | — | [44100] | 0.423s |
| rate-weak | EXACT | 0 | [44100, 48000] | 0.355s |
| channels-strong | OBSERVATION | — | [44100] | 0.307s |
| channels-weak | EXACT | 0 | [44100] | 0.294s |
| rg-track | INTEGER-GAIN MODEL EXACT | 319074 | [44100] | 0.176s |
| rg-album | INTEGER-GAIN MODEL EXACT | 319074 | [44100] | 0.188s |
| rg-missing | EXACT | 0 | [44100] | 0.149s |
| rg-album-fallback | INTEGER-GAIN MODEL EXACT | 318696 | [44100] | 0.140s |
| rg-clipping-prevention | EXACT | 0 | [44100] | 0.131s |
| high-precision-track | EXACT | 0 | [44100] | 0.164s |
| high-precision-album | EXACT | 0 | [44100] | 0.194s |
| delayed-dump | EXACT | 0 | [44100] | 1.465s |
| timed-normal | OBSERVATION | — | [44100] | 4.310s |
| timed-delayed | OBSERVATION | — | [44100] | 4.207s |
| timed-delay-overrun | OBSERVATION | — | [44100] | 5.429s |
| timed-no-prefetch-delay | OBSERVATION | — | [44100] | 5.342s |
| remove-early | OBSERVATION | — | [44100] | 4.165s |
| remove-late | PRECONDITION MISSED | — | [44100] | 6.192s |
| remove-clock-near-boundary | OBSERVATION | — | [44100] | 4.543s |
| remove-clock-after-boundary | OBSERVATION | — | [44100] | 4.278s |
| pause-delayed | OBSERVATION | — | [44100] | 2.198s |
| stop-delayed | OBSERVATION | — | [44100] | 2.242s |

- PCM output is untimed; exact dump equality does not establish real-time device gaplessness.
- Timed null observations establish engine/control behavior, not actual output samples or DAC continuity.
- The harness exercises libmpv through ctypes, not the future Rust FFI adapter.
- Lossy trimming, real Plex URLs, authorization expiry, hardware paths and rate-change muting are not measured.
