# Application patch: bounded playback buffers and image codecs

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

No platform-specific upstream implementation was removed. Recheck these patches
when upgrading; `player_smoke` verifies playback, pause/seek, sink limits and
file/worker release without opening a window.
