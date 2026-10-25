# Playback timing decision prototype

Open `index.html` in Brave (or double-click it): one self-contained logic demo, no server, audio, credentials or persistence. Free-play controls and sixteen tabbed walkthroughs expose the state and mock reporting sink. [CONTRACT.md](CONTRACT.md) records the proposed adapter seam and limits; the canonical decision is [Choose playback occurrence timing and buffered-edit behavior](https://github.com/42lizard/qoda-player/issues/20).

Native measurements use quiet synthetic WAVs, libmpv, the existing PipeWire route and the explicitly selected built-in `hw:CARD=avsmax98927,DEV=1` PCM at 48 kHz/S16/stereo. They do not use Plex or change mixers, services, device defaults or desktop configuration. Direct-device preflight fails if busy/unsupported; it does not force another application's release or silently fall back. The ALSA tap is throwaway instrumentation derived from audio qualification `4042dca`, not a deployment requirement.

```sh
python3 prototypes/playback-timing/qualify.py
```

Requires Cargo/Rust, Python, libmpv/libasound, Node for replaying the HTML module, and the measured audio routes. The runner captures clocks/edits, replays the walkthroughs and summarizes the evidence, then keeps complete traces as gzip beside sampled readable JSON. Individual measurements: `measure.py --modes alsa pipewire --edits`; `probe.py` owns one bounded libmpv process. Every child terminates/destroys its handle; there are no long-lived terminals, servers or user services. Fixtures/raw PCM remain in owner-only temporary directories for diagnosis; public evidence redacts their paths.

The case replayer uses the exact pure model and steps in the HTML. It captures model states for review; it is not a production test suite or proof of a Rust implementation. Physical input is not inferred from it. Browser headless preview attempts on this host timed out and were stopped; those attempts are not recorded as successful visual review.

The full traces include an initial ALSA-busy series, one later native short-start stall, and an early-edit observer that waited for an extra EOF after removing an entry. The corrected early runs are separate evidence. One rebuild's strict checkpoint comparison differs by one frame; its exact source-aligned suffix comparison is recorded separately. Do not turn these into an all-cases-passed or generic sample-exact/physical-output claim.

The useful accepted-buffer removal case had already handed B frames to ALSA before B's nominal boundary; post-reset payload was exactly A's recovered tail then C. PipeWire event/control behavior was measured, but its downstream reset waveform was not captured. The contract's uncertainty branch covers that unqualified boundary.

This branch is throwaway planning evidence, with no PR/merge. No playable production application is delivered here.
