# Implementation queue

The accepted product decisions remain in [the planning map](https://github.com/42lizard/qoda-player/issues/1). These implementation tickets turn them into reviewable increments; [first-release acceptance](https://github.com/42lizard/qoda-player/issues/12) remains open.

| Work | Ticket | Dependency and evidence |
| --- | --- | --- |
| Approved layout A hierarchy | [#22](https://github.com/42lizard/qoda-player/issues/22) | Left library navigation, center browser, persistent wide queue, narrow queue drawer, bottom transport and menu. Fictional offscreen renders plus transport input tests. |
| ReplayGain | [#23](https://github.com/42lizard/qoda-player/issues/23) | Metadata provenance, album-session policy, immutable per-occurrence preparation and measured per-file gain. Next audio implementation. |
| Audio qualification | [#27](https://github.com/42lizard/qoda-player/issues/27) | Develop alongside gain; verify sample precision/count/order and transitions before qualified labels. Physical output remains unverified. |
| Direct ALSA fidelity | [#24](https://github.com/42lizard/qoda-player/issues/24) | Explicit device, exact negotiation and conversion refusal; depends on the qualification gate. |
| Current-stream recovery | [#25](https://github.com/42lizard/qoda-player/issues/25) | Bounded retries of the pinned version with position, pause intent and generation fences preserved. |
| Plex reporting | [#26](https://github.com/42lizard/qoda-player/issues/26) | Confirmed nominal playback only; live API qualification remains in [#16](https://github.com/42lizard/qoda-player/issues/16). |
| Remaining approved interactions | [#29](https://github.com/42lizard/qoda-player/issues/29) | Cross-library search, drag ordering, focus review and further visual refinement; physical input/multi-output acceptance remains in [#18](https://github.com/42lizard/qoda-player/issues/18). |
| Offline startup, history and packaging | [#28](https://github.com/42lizard/qoda-player/issues/28) | Split into focused tickets before implementation. Permanent desktop/service installation remains opt-in. |

Playback currently uses Desktop/PipeWire with ReplayGain off. Creating an audio ticket does not enable its feature or qualify hardware output. Development checks use fictional data and synthetic audio; real music stays stopped unless explicitly started by the user.
