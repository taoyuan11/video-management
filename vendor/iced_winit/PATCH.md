# Windows 11 composition window patch

Source: iced_winit 0.14.1 from crates.io, upstream iced revision
`38237dd294da3256c2ac4d3f8288fc73956c91e0`, directory `winit/`.
The upstream MIT license is included.

Only `src/conversion.rs` differs from upstream: Windows transparent windows
set `WindowAttributesExtWindows::with_no_redirection_bitmap(true)` when using
DX12 + Visual presentation. An explicit software-renderer override keeps the
GDI buffer and opaque fallback.

This enables `WS_EX_NOREDIRECTIONBITMAP` at creation and leaves winit in charge
of preserving it during resizing, maximization, and restoration. Without it,
the default GDI redirection surface obscures DWM's Acrylic behind the
DirectComposition visual. It also avoids winit's legacy empty blur-region path.

This patch is used together with `../iced_wgpu/PATCH.md`. Opaque windows and
other platforms retain upstream behavior. Remove it when iced exposes the
no-redirection window attribute through its public platform settings.
