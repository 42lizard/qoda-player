# Independent Rust backend with embedded libmpv

Qoda uses a separate user-scoped Rust/Cargo backend to own the listening session, provider access, storage, desktop media controls, and embedded libmpv; Quickshell/QML presents its state through a versioned local socket interface. This lets playback outlive a UI reload or crash and keeps provider and playback orchestration behind small interfaces without requiring a custom native Quickshell plugin. The tradeoff is an explicit reconnect/protocol contract and shared failure lifetime between libmpv and the backend; backend restart restores the session paused.

The canonical ownership, communication, concurrency, and validation contract is recorded in [Choose runtime architecture and module boundaries](https://github.com/42lizard/qoda-player/issues/9). This Rust/Cargo decision supersedes the initial C++/Qt/CMake proposal.
