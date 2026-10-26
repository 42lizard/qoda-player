# Shutdown persistence outcomes

The backend releases audio and resources, attempts the final session checkpoint,
and reports the result through its exit status. Socket removal and lock release
still run when saving fails. These outcomes apply to IPC/UI/MPRIS Quit and the
SIGINT used by the installed backend unit; they do not grant listening credit or
claim hardware presentation.

| Exit | Meaning | Installed launcher |
| --- | --- | --- |
| 0 | Normal shutdown; final checkpoint completed. | Quit succeeds after units stop. |
| 74 | Final checkpoint failed or could not be confirmed. | Quit returns failure with an unsaved-changes diagnostic. |
| 75 | Session committed; backup maintenance failed. | Quit reports a backup warning without calling the session unsaved. |
| 76 | Persistence completed, but runtime socket cleanup failed. | Quit reports cleanup failure; conflicting paths are preserved. |

If persistence and socket cleanup both fail, the persistence result takes
precedence and the additional cleanup error is logged. Other unhandled failures
retain their ordinary failure status; startup/fatal compatibility classification
is still part of #45.

`qoda status` reports `last_backend_exit` from the backend unit's `Result`,
`ExecMainCode` and `ExecMainStatus`, alongside service activity and release
identity. It never starts the services or connects to the backend. Status names
are `clean`, `checkpoint_failed`, `backup_warning`, `runtime_cleanup_failed`,
`not_started`, `not_installed` and `unconfirmed`. A signal death, absent result or
unknown exit status cannot certify a checkpoint. This is the last unit invocation,
not a durable audit history and not proof that a running process is healthy.

`qoda quit` reads that result even when the stop job itself succeeds (or fails).
It does not start/retry anything after an error. The backend unit uses
`RestartPreventExitStatus=74 75 76`, so orderly shutdown failures do not resurrect
the player. The development runner also propagates these codes for a backend it
owns. Existing installed units must be upgraded through the stopped-only installer
to receive this policy; these source changes do not alter the active desktop.

A failed save can leave recent in-memory changes unsaved. Inspect
`journalctl --user -u qoda-backend` before restarting or restoring an available
backup. A backup warning means the current session committed; it does not certify
that backup retention/creation completed. Existing account cleanup and normal
backup retention policies still apply. Quit does not add a recovery-data purge.

## Evidence and remaining scope

Real-process tests start an empty paused backend in private XDG directories with
an unavailable private session bus, then exercise IPC Quit and SIGINT. A held
SQLite write transaction blocks a changed session's final save: the process exits
74, removes its socket, and leaves the previous committed database readable and
unchanged. A replacement file, directory or symlink at the socket path causes exit 76
and remains untouched: cleanup checks the socket type and recorded device/inode.
A subsequent startup refuses these conflicts rather than deleting them. A storage-worker test commits a fictional nonempty queue while backup
permissions are unsafe; the warning maps to 75, the session remains readable and
the previous backup remains intact.

Python fixtures verify exit classification, successful stop jobs with failed
checkpoints, failed stop jobs, signal/unknown results, development-runner exit
propagation and the actual installed launcher behind a private manager stub.
Native unit syntax checks include the restart-prevention policy. No fixture loads
media, contacts Plex, accesses credentials or changes host services.

Actual user-manager observation across graphical logout, service crash/retry,
concurrent activation and UI loss remains #45. Absent-UI MPRIS activation and the
installed bar-launch path also remain open. These tests do not replace that
qualification. Safe in-use updates/rollback remain #46.
