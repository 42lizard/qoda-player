# Nominal presentation owns current playback identity

Status: accepted. Canonical decision: [Choose playback occurrence timing and buffered-edit behavior](https://github.com/42lizard/qoda-player/issues/20).

Decoder/file events can precede nominal output presentation. Accepted PCM may later be discarded. Therefore the Playback adapter exposes preparation, engine selection, presentation confidence and output invalidation separately; Session remains the authority for current membership, visits, history and intent.

Use qualified nominal clock evidence, with exact predecessor spans only when their provenance supports that mapping. Missing evidence produces explicit uncertainty rather than inferred listening. Queue edits that affect buffered output require a classified reset/rebuild boundary before acknowledgement. Intent generations and output epochs have separate roles, so an old recovery cannot resume after Pause and a seek cannot turn discarded buffers into listening.

The detailed interface, reporting defaults, evidence, failure cases and supported boundaries live in the linked decision and its throwaway artifact. A generic hardware/audibility clock, arbitrary downstream retraction and a production adapter are not inferred from the prototype. Real Plex reporting and physical output remain explicit release acceptance work.
