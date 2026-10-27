# Arch workflow package

`tools/build-arch-package.sh [output-directory]` invokes this PKGBUILD as an
unprivileged user against the checkout. It needs the native development libraries,
Cargo/Rust compatible with the workspace, makepkg and fakeroot. It does not install
packages or invoke a service manager. Runtime dependencies are in the PKGBUILD;
Quickshell is available in the qualified Arch/Omarchy repositories. The build uses
Cargo.lock. Distribution native LTO is disabled because ring's GCC objects cannot
be consumed by Rust's LLD without cross-toolchain LTO setup, as described in
[Arch's Rust packaging guidelines](https://wiki.archlinux.org/title/Rust_package_guidelines#Build).
Rust release optimization remains enabled.

Versions combine the workspace version, full-history revision count and abbreviated
commit; CI checks out full history. COMMIT and SHA256SUMS accompany the archive;
release.json records the full source commit and storage-reader compatibility.
Build paths are remapped in Rust. The package is an x86_64 workflow artifact, not
an AUR submission or a signed pacman repository.

The root-owned payload is under /usr/lib/qoda-player/releases, selected through a
package-owned current symlink. It reuses the source launcher's logic while separate
qoda-packaged-* units prevent a user source unit from taking precedence. Explicit
package-unit overrides are refused. The backend accepts root-owned executable UI
launchers only through protected root-owned directory chains; ordinary user-owned
source launchers retain their existing checks. Unit generation asserts its expected
source templates before rewriting them, so upstream changes cannot silently omit
the compatibility entry point. No unit is enabled or started by installation.

A PreTransaction AbortOnFail hook refuses upgrades/removal while packaged processes
are running. Quit all sessions first. It does not stop users' processes or inspect
private profiles as root. At user startup, the backend entry checks that user's
profile against the release's supported schema evidence before opening it. General
pacman downgrades never restore or downgrade databases.

`tools/check-arch-package.sh archive.pkg.tar.zst` checks a real archive in a private
root. On an unprivileged host it uses a user/PID/network namespace with a read-only
host filesystem and one writable temporary directory. In CI it runs inside the
job's disposable root container. Tests inspect metadata/dependencies/layout, start
the actual packaged backend with an empty private XDG profile and unavailable bus,
and verify refusal of unsupported settings without altering them. A privileged CI
check additionally drops to UID 65534 to test protected system-launcher acceptance
and writable-ancestor refusal with the packaged binary. No configured launcher is
executed by that startup check. Local single-UID namespaces cannot run that subcase;
CI requires it explicitly.

Real pacman install/upgrade/remove transactions retain a synthetic user-data sentinel.
The upgrade fixture repacks the real archive with a bumped package version and a
changed documentation file; this tests replacement/ownership without another Rust
build. The process guard is separately tested through its CLI with a process-tree
fixture. Its hook is masked only in the empty transaction root, which deliberately
has no Python interpreter; package scriptlets never run. These are not actual
user-manager, graphical-session, logout or physical-audio qualifications.

Artifact upload uses a pinned [upload-artifact release](https://github.com/actions/upload-artifact/releases/tag/v7.0.2).
See [the user guide](../../docs/user-guide.md) for installation and coexistence.
