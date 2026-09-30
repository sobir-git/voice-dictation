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

## Dictation latency

At INFO, `worker_dispatch` records worker queue time and model reuse;
`since_request_ms` separately measures elapsed time since stop/retry, including
saving and preparation before worker admission. `model_available` records loading
wait and the actual runtime profile.
`inference_attempt` identifies the first user attempt after load, model readiness
age, and time since the previous attempt. The silent load warmup is logged
separately and does not count as a user inference attempt.

`native_timing` records backend/model loading, session creation, warmup,
stream start, each processed audio chunk, and finalization. Chunk records include
feature extraction (`mel_ms`), encoder (`encode_ms`), decoder (`decode_ms`),
wall time, received samples, committed audio and buffered audio. Counters are
reported as deltas, excluding warmup and previous streams. Before-call markers
and supervisor wait warnings retain evidence when a call does not return.
`receiving_operation` and `receiving_sequence` identify the supervisor IPC request
that received a diagnostic; `stage` identifies the measured native work. Stream
initialization telemetry precedes its acknowledgment. `snapshot_ms` measures the
public timing accessor's transcript materialization, outside native call wall time.
Cheap feeds skip that accessor and skip DEBUG context construction at INFO.
Retries include audio preparation and native batch timings; Whisper reports VAD,
feature extraction and generation separately.

The native encoder counter measures the forward compute call, including work
performed by the Vulkan backend there. It is not a GPU hardware timestamp.
`unaccounted_ms` is call wall time minus the measured native stages, clamped at
zero for timer rounding. It includes graph preparation, buffer allocation,
transfers outside compute, result handling and other uninstrumented work. It must
not be described as shader compilation or GPU preparation without further
native/driver evidence. Native zero values can mean unmeasured work.

`audio_queue_timing` separates microphone-to-worker queue age from IPC/inference
call time. INFO retains calls of at least 10 ms and queue ages of at least 100 ms;
DEBUG retains every audio request and completion. `first_preview` records the
first nonempty result without logging its text. `stream_timing_summary` retains
maximum queue/call times, feed/sample counts and outcome, including cancellation
and failure. `dictation_stage` records each coordinator stage duration and elapsed
time since recording stop; `dictation_terminal` includes the last delivery stage.
These records share daemon session, job ID and native worker PID. Logging stays
asynchronous and produces no periodic inference telemetry while idle.

The explicit native timing probe uses temporary configuration, a supplied cached
Parakeet GGUF, and the bundled synthetic speech fixture. It runs two paced short
streams against one resident worker; it neither records a microphone nor writes
history or inserts text:

```sh
VOICE_LATENCY_MODEL=/path/to/cached/parakeet.gguf cargo test --locked --features vulkan \
  --lib inference::tests::native_latency_probe -- --ignored --exact --nocapture
```

Omit `--features vulkan` for the CPU path. The probe prints numeric timing records
and checks stage counters, warmup separation, job correlation and worker reuse.

## Idle warmup and paging

Recording admission asks the existing resident Parakeet child to exercise the
same 33,280-sample silent warmup as loading when at least 60 seconds have elapsed
since successful real inference completion or successful warmup completion.
Fresh load warmup already counts as warm. A long recording refreshes the clock
at final completion, not at recording start. Failed and cancelled calls do not
advance it. There is no idle timer, heartbeat, or per-recording unconditional
warmup. Batch recording modes also request warmup at Start, before Stop.

Warmup owns a separate native stream (or a separate run for `vulkan-full`), drops
its synthetic state before real stream creation, and never produces previews,
history, text output, or a user inference attempt. The microphone capture thread
continues independently; real audio queues behind warmup. Existing queue limits
and saved-recording recovery still apply if a slow call fills the queue. Warmup
uses the same supervised 120-second call deadline and cancellation checks as real
inference. A failed child is discarded through the existing worker recovery path;
loading/fallback rules remain unchanged.

Every recording admission logs one `idle_warmup_decision` with
`idle_since_completion_ms`, `threshold_ms: 60000`, `needed` (boolean),
`decision: needed|skipped`, worker PID and job correlation. This includes the
fresh-load skip and uses completion age even after a long recording;
`since_previous_attempt_ms` still describes attempt start age separately.

`native_timing` stages `warmup_start`/`warmup` describe load warmup;
`idle_warmup_start`/`idle_warmup` describe recording-start warmup. Idle warmup
records carry `receiving_operation: idle_warmup`, request sequence, worker PID
and the recording job ID. Warmup wall time excludes paging reads and diagnostic
emission. Native chunk/finalization wall time is captured before after-snapshot
reads and IPC writes; `batch_stream_inference` sums native stream creation, feed
and finalize wall time, excluding telemetry between calls.

`paging_snapshot` records have `stage`, `phase` (`before`/`after`), native
`process_pid`, correlated supervisor `worker_pid`, job and receiving request
identity. `memory` contains `vm_swap_kb`, `rss_kb`, `rss_anon_kb`, `rss_file_kb`,
absolute `minor_faults`/`major_faults`, `host_mem_available_kb`,
`host_swap_total_kb`, and `host_swap_free_kb`. Top-level
`minor_faults_delta`/`major_faults_delta` compare the after and before snapshots;
before deltas and missing or malformed fields are null, not zero. After records
include `success` where the call outcome is available; batch call framing also
includes `native_wall_ms`. These are cheap `/proc/status`, `/proc/stat` and
`/proc/meminfo` reads, not `smaps` walks. They run only on the inference worker.

Snapshots frame load/idle warmup, whole real attempts (`stream_attempt` or
`inference_attempt`), significant native stream feeds (`stream_feed` or
`batch_stream_feed`), finalization (`stream_finalize` or
`batch_stream_finalize`), native batch calls (`batch_inference` or
`batch_chunk_inference`) and Whisper generation (`whisper_generate`). Streaming
sampling uses the first 33,280 received samples and subsequent 16,640-sample
boundaries, independent of text/commit progress; tiny feeds do not read `/proc`.
The supervisor's stalled/failed `worker` snapshot also includes the same `memory`
absolute counters and host availability, so a hung child need not return to
provide paging evidence. A killed child cannot guarantee an after snapshot.
No audio or transcript content is added by this telemetry.

Major faults indicate disk-backed page-in and are **not proof of anonymous
swapping**: file-backed model mappings can also fault. `VmSwap` measures process
anonymous swap usage, not model file eviction or GPU residency. GPU allocations
can be outside process RSS; integrated GPU accounting can share host RAM.
Use both paging counters and native timings to investigate a stall, without
attributing all unexplained wall time to swapping or shader compilation.

Swap is diagnostic only, not an additional warmup trigger. Existing measurements
in `tools/model_speed_experiments/memory-summary.json` show substantially different
CPU RSS and GPU residency, but do not establish a swap threshold that predicts
latency. A persistent nonzero `VmSwap` would otherwise cause perpetual warmups.
The 60-second decision is deterministic; evaluate it with a resident release
worker and an actual 61-second idle interval on the target GPU host. The ignored
synthetic native probe privately ages the completion clock instead of waiting,
and compares identical real fixture text with/without warmup on the same child.

Both fresh autostart setup and `update_local.sh`'s existing-service drop-in set
`MemorySwapMax=0` for the daemon service and its inference children. This is
service-specific: no global swapoff, hard memory limit, or `mlockall` is used.
No `MemoryLow` is added: there is no measured protection budget/effectiveness,
and user-service ancestors can restrict protection. Swap exclusion can increase
reclaim pressure, file-backed faults, and OOM-kill risk when physical memory is
insufficient; it does not pin model pages, protect GPU allocations, or prevent
OOM. Check `systemctl --user show speech-to-text-daemon.service -p MemorySwapMax`
and the effective service cgroup `memory.swap.max` after an authorized install.
On hosts without applicable cgroup v2 swap accounting/controller delegation,
systemd can ignore the resource control; inference still works and missing
`/proc` fields remain null. The setting is not a promise of enforcement on those
hosts. This task changes installer sources only, without installing or restarting
the active service.
