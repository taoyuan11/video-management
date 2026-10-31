# Application patches: playback compatibility, bounded buffers and image codecs

Source: `iced_video_player` 0.6.0 from crates.io, upstream commit
`121b77de686d49e7bc768771b1a1c1c18f50c51f`,
https://github.com/jazzfool/iced_video_player. MIT / Apache-2.0 licenses retained.

- Disable iced's implicit default features and use `image-without-codecs`.
  The application selects its renderers (including tiny-skia fallback) and PNG
  support. Video decoding remains in GStreamer; unrelated raster codecs and
  their encoders no longer enter the application through this dependency.
- In `Video::from_gst_pipeline`, cap video and subtitle appsinks at two buffers,
  drop old buffers when full, disable last-sample retention and EOS waiting.
  The previous `drop=true` with the default unlimited queue did not bound memory.
  These settings apply before playback, including to `Video::new`.
- Omit upstream examples and development-only files from the vendor copy.
- Accept GStreamer's variable/unknown framerate (`0/1` or omitted) and retain
  timestamp-based scheduling. Decoded buffers without a duration still reach
  the renderer; subtitle matching uses their presentation timestamp.
  `Video::framerate()` returns 0 when there is no fixed nominal rate.

No platform-specific upstream implementation was removed. Recheck these patches
when upgrading; `player_smoke` verifies playback, pause/seek, sink limits and
file/worker release without opening a window.
Run `cargo test --locked -p iced_video_player --lib` for regression coverage of
fixed, variable and unspecified rates, missing buffer duration, actual frame
delivery to the renderer's storage and pipeline shutdown.

## Playback responsiveness (2026-10-30)

- Load into PAUSED and require a decoded preroll, checking cancellation and a
  ten-second deadline. Assign URI as a property and cap libav decoder threads
  at four. UI loads stay silent until accepted; the application independently
  bounds the whole load, including native calls that ignore cancellation.
- Serialize state changes, seeks, properties, bus draining and position queries
  on a dedicated control worker. UI getters use cached state, commands coalesce
  in a fixed-size mailbox, and no shared UI mutex spans a native operation.
  Errors are polled even when the widget is paused/hidden. A ten-second control
  heartbeat and twenty-second joint frame/position stall watchdog report failures.
- Stop and Drop signal workers without joining native decoder teardown. The
  application retains and reaps retired videos/loads and limits outstanding
  jobs to four. `shutdown(timeout)` supports bounded CLI/test cleanup. A native
  call stuck inside Windows or a codec cannot safely be forcibly terminated as
  a Rust thread: it retains one bounded slot until it returns or the app exits.
  This is in-process containment, not decoder process isolation.
- Remove pipeline state queries from the frame consumer and throttle empty/EOS
  pulls. Copy validated NV12 planes on that worker into CPU-owned frame storage;
  GPU upload holds no GStreamer map or frame mutex. Respect plane offsets,
  padded strides, odd dimensions and resolution changes; reject malformed or
  excessive buffers before allocation/upload. Optional subtitle metadata cannot
  suppress otherwise valid images; unexpected frame-worker exit reports failure.
- Replace unbounded thumbnail spinning with a bounded sleep/wait. Explicitly
  wait for shutdown in command-line playback and file-release tests.

Regression coverage also includes blocked native seek/teardown, cancellation
without preroll, hidden/paused bus errors, watchdog expiry and padded/odd frames.
Application tests cover stale successful loads, bounded retries, actual H.264
pause/seek/mute/volume, EOS replay, repeated file release and corrupt/missing files.

## Stable dependency update (2026-10-31)

- Update GStreamer Rust bindings from 0.23 to 0.25, GLib from 0.20 to 0.22,
  and thiserror from 1 to 2. Keep the application's bindings on the same versions
  to share pipeline, sample and error types. The native SDK is GStreamer 1.28.8.
- Use GLib's separate typed property check for the decoder thread limit;
  retain the untyped availability check for optional A/V synchronization.
- Retain the upstream 0.6.0 version, source attribution, platform code and all
  application playback patches; iced 0.14 remains the latest stable release.
