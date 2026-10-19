# scripts/

Windows screenshot and window-automation helpers used during development and
acceptance runs (PowerShell, no third-party dependencies). These are manual
tools only — not part of the build, tests, or the app runtime.

| Script | Purpose |
| --- | --- |
| `capture_hwnd.ps1` | Capture a specific window by title (`PrintWindow`) |
| `capture_hwnd_dpi.ps1` | Same, output at physical pixels (DPI-true resolution) |
| `capture_window.ps1` | Wait for a window by process name, then capture |
| `capture_pages.ps1` | Page through `render_probe` with PageDown, capturing each page |
| `winshot.ps1` | Move the AiCouncil window to a secondary display (if any) and capture its rectangle |
| `window_to_screen.ps1` | Move the window to (10,10) and resize to 1500×900 (shot composition) |
| `find_window.ps1` | Print target window handle and coordinates (debug locating) |
| `click2.ps1` | Simulate a segmented move-and-click at client-area relative coordinates (drive the UI) |
| `click_at.ps1` | Click at client-area relative coordinates in DPI-true pixels |
| `screenshot.ps1` | Delayed full-screen capture |
| `crop.ps1` | Crop a region of a PNG |
