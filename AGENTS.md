# Project instructions

## Supported platform

- This application targets Windows 11 x64 only. Windows 10, macOS, and Linux are out of scope.
- Use Windows APIs and Windows 11 fonts directly where appropriate; do not add cross-platform abstractions or legacy OS compatibility paths.
- Keep fallbacks for capabilities that can be unavailable on Windows 11, such as DWM transparency and graphics support.
- Preserve upstream platform code in vendored dependencies unless a specific application change requires modifying it.

## Design system

- Read [design.md](design.md) before adding or changing UI. It defines reusable visual, layout, control, and accessibility rules, not product features or page workflows. Update it when shared design rules change; keep product-specific behavior out of it.
- Follow the Windows 11 Fluent light and dark styles: native Desktop Acrylic, theme-appropriate translucent surfaces, blue accent, soft corners, subtle borders and shadows, and opaque text, icons, and video.
- Reuse colors, typography, Segoe Fluent Icons, and control styles from `src/ui/fluent.rs`. Centralize shared visual rules instead of introducing competing styles.
- Consult the existing implementation and [README.md](README.md) for product behavior, page structure, and supported window sizes. Preserve existing navigation, collapsible details, contextual commands, and minimum window sizes unless the task calls for changing them; these are not defined by `design.md`.
- Keep native window behavior and file pickers, bundled Source Han Sans typography, and the opaque fallback when DWM material or graphics transparency is unavailable on Windows 11.
- Use concise Chinese copy and consistent product terms. Provide visible focus and disabled states, text alongside status colors, and contextual empty states. Do not add hover text tooltips to buttons, including icon-only actions. Preserve keyboard shortcuts and path confirmation for recycling files.
- For UI changes, review the code against `design.md` for layout, long content (including paths), control states, and theme consistency. Check the minimum window sizes defined by the application, and the opaque fallback when changing materials. The user will perform visual and interactive UI checks.

## Verification

- After changing code, perform appropriate code checks and automated tests, but do not control the computer or automate the desktop to inspect the UI. The user will check the UI themselves.

## Service lifecycle

- Track every long-running process or service started during the current task, including development servers, file watchers, containers, tunnels, and background jobs.
- Stop each process or service as soon as it is no longer needed.
- Before sending a final response, verify that every process or service started during the task has stopped. Never leave a Codex-started service running after the task or conversation ends.
- Only stop processes or services started for the current task; do not stop pre-existing user processes.
