# Windows 11 transparent composition patch

Source: iced_wgpu 0.14.0 from crates.io, upstream iced revision
`3997291f318a8bc06fa522f5579836fb3feb94df`, directory `wgpu/`.
The upstream MIT license is included as LICENSE.

Only `src/window/compositor.rs` differs from the upstream source:

1. Read `wgpu::BackendOptions::from_env_or_default()` so the application's
   `WGPU_DX12_PRESENTATION_SYSTEM=Visual` selects a DirectComposition visual.
   The stock HWND swapchain only supports opaque composition; setting DWM
   Acrylic alone therefore produces a black client background.
2. Prefer premultiplied surface alpha on Windows, matching iced's composited
   render target and the DirectComposition swapchain.
3. Expose `Compositor::supports_transparency()` after successful device
   initialization. The app keeps an opaque backdrop if the selected renderer
   does not preserve alpha, including the software-renderer fallback.

The app sets DX12 and Visual only for its own process, before creating any
threads. Explicit developer environment overrides are preserved. No system
graphics or transparency setting is changed. The existing GPU video renderer
continues to use the same patched iced_wgpu instance and device.

Remove this patch after upgrading to an iced version that exposes backend
options and alpha capability directly. Recheck multiple windows, rounded
corners, maximization/restoration, video playback, full screen, and fallback.
