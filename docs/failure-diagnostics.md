# Dictation failure diagnostics

See [architecture.md](architecture.md) for the implemented ownership boundaries,
lifecycle, worker supervision and fault probes.

Inspect both the configured log file (normally
`~/.local/share/speech-to-text/app.log`) and
`journalctl --user -u speech-to-text-daemon.service`. Native engine stderr and
systemd termination reasons remain in the journal. Logs alone cannot reconstruct
a process killed before it could write its final event.

Follow `job` IDs within a daemon PID. Admission, stage transitions and the single
terminal event explain which work remains active. A `history` ID links a saved
recording to its inference result. Worker start/stop lines identify process
replacement. State reports also include the active job IDs and stages.

INFO includes lifecycle transitions, persistence and worker events. Errors retain
full Rust error chains. DEBUG adds command names and client IDs without command
payloads; dependency HTTP/TLS debug messages are filtered. Transcript text and raw
audio are not added to these logs.

Historical evidence inspected on 2026-09-13 included decoder generation-cap
failures on September 9, repeated microphone-silence failures, and capture starts
without completion before daemon exit. Those older logs did not record indicator
state delivery, so they cannot establish which path caused a particular stuck
indicator.

## Diagnostic reports

In **Status**, **Copy report** copies the current summary; **Export report** saves
an owner-readable ZIP under `~/.local/share/speech-to-text/diagnostics/`. Exports
run on a separate thread so journal collection cannot hold up recording storage.
Only one export is admitted at a time. They are local files, never uploaded.

When the app cannot start, run:

```sh
python3 tools/diagnostics.py --output /path/to/new-report.zip
```

For a custom logging directory, pass `--log-path /path/to/app.log`. The bundle
contains current and rotated daemon/component logs, a daemon state snapshot,
installed binary SHA-256 hashes and ELF notes, seven days of available service
journal and matching kernel messages, systemd exit details and core-dump listings.
It does not contain history databases, recordings, models, environment variables
or raw core dumps. Native stderr and older logs can contain sensitive error text
or paths: review a bundle before sharing it.

Each log contributes at most 4 MiB, with a 48 MiB total log budget, newest files
first. The manifest identifies missing files, byte counts and truncation. System
commands have five-second deadlines and report permission errors, missing tools,
nonzero exits and truncation. Existing exports are never overwritten. The small
in-app summary remains an excerpt; use the ZIP for an incident investigation.

## Logging contract

New records are JSON lines with schema version, wall-clock milliseconds,
monotonic elapsed milliseconds, process/session identity, component, thread,
severity and contextual identifiers. Desktop requests carry a client-session ID
and request ID; daemon command logs link these to the connection and admitted job.
Async storage replies retain request identity, and worker logs identify the job
and native child PID. Audio levels and transcript payloads are not logged by the
new UI/request instrumentation. Native stderr is captured as bounded text records
with its worker PID; it is also included in the daemon journal.

A bounded 2,048-record queue feeds a dedicated writer. Disk and stderr writes do
not run on the UI or coordinator thread. Overflow and write failures are counted,
reported on subsequent writer activity, and exposed as `logging_health` in
Diagnostics. A failed file sink is retried on later events. Normal exit and panic
handlers request a drain with a two-second deadline; panics include a backtrace.
Each message is limited to 16 KiB and marked when truncated.

The daemon keeps five backups beside the current log. Desktop, HUD, adapter and
CLI processes have separate PID files to avoid competing rotation. The ten newest
completed component log families are retained; active process files are preserved.
New log files and ZIP exports have mode 0600. Existing configured log locations
and user configuration remain unchanged.

## UI and crash evidence

Desktop logs distinguish receipt, application of state and the paint callback.
HUD messages carry a sequence number; its paint callback sends that sequence back
to the companion. While visible, a one-second check requests fresh evidence; a
three-second missing acknowledgement logs a timeout and reaps the HUD, with the
existing bounded retry delay. There is no heartbeat when the indicator is idle.
A paint acknowledgement proves the widget reached its paint callback, not that
the compositor physically displayed the pixels. Native screenshots remain the
visual check.

Native child reaping records exit code, terminating signal, core-dump flag and
whether the supervisor requested a kill. OS-provided core metadata is collected
when available; this does not enable unrestricted memory dumps or alter host core
policy. Binary hashes and ELF build IDs identify the exact executable. Release
binaries are stripped, so detailed symbolication still needs matching debug
symbols. No logging system can guarantee evidence after SIGKILL, power loss,
exhausted storage, an unavailable OS journal or a compositor failure. These limits
are explicit rather than represented as successful logging.

## Recording preservation

Capture creates an owner-readable `dictation-*.wav` directly under the data
`recordings/` directory before starting the recorder. Cleanup is disabled at
creation. The configured legacy `audio.temp_file` value is preserved but no
longer chooses the capture location. Audio is flushed to disk every second while
recording and synchronized at finalization. No periodic work runs while idle.

A client disconnect finalizes and transcribes the captured audio. Cancel or abort
stops processing/output but retains captured audio as a failed history entry for
retry. Late capture and storage completions still preserve and index audio.
Finalization, inference, output, and SQLite failures never authorize deleting the
capture file. Malformed and silent captures are retained too.

Before admitting new captures at startup, the daemon scans unindexed
`dictation-*.wav` files, repairs unfinished WAV size fields when possible, and
adds failed history entries without automatically typing recovered text. This
also recovers captures left by a killed daemon. Unknown headers remain intact
and are indexed for inspection. Recovery is idempotent by audio path. Automatic
retention applies only to successfully transcribed, non-favorite recordings;
failed and unindexed files are protected. Explicit history deletion remains a
user action that deletes audio.

The phone bridge continuously drains daemon telemetry between Start and Stop;
it retains only bounded handshake state, not streamed previews. This prevents
long recordings from exhausting the control socket buffers.

Power loss can lose writes not yet synchronized, and a full or failed disk can
prevent new audio from being written. Audio that never arrives from the microphone
cannot be recovered. These limits do not permit deleting audio already captured.
