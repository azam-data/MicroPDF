# Repository Guidelines for Agents & Contributors (`AGENTS.md`)

Welcome to **MicroPDF** — a fast, ultra-lightweight, portable PDF viewer and power editor written in Rust. Every change in this repository MUST respect the architectural principles and documentation rules below.

---

## 1. Core Architectural Constraints (Non-Negotiable)

- **Zero GPU, Zero GUI Toolkit**: All UI chrome, dialogs, previews, and overlays are CPU-rendered directly into the `softbuffer` `0x00RRGGBB` pixel slice using `src/viewer/ui_draw.rs`.
- **Zero Idle RAM Bloat**: Inactive tabs hold zero rendered bitmaps; modals/overlays allocate nothing when closed; thumbnail and tile caches remain strictly bounded.
- **Zero OS Breadcrumbs / Portable-First**: Keep `micropdf.ini` next to the executable (OS config fallback only if read-only). Load PDFium strictly from the executable folder (or `MICROPDF_PDFIUM_DIR` for development).
- **Dependency Discipline**: Avoid adding new Cargo dependencies whenever the existing stack (`winit`, `softbuffer`, `pdfium-render`, `lopdf`, `image`, `flate2`, `ab_glyph`, `rfd`) can solve the task cleanly.
- **Zero Warnings Across All Targets**: `cargo check --tests` and `cargo build --release` must produce **0 compiler warnings** on Linux, Windows (x64), and macOS (arm64).

---

## 2. MANDATORY Rule: Keep the Built-In User Guide & Shortcuts Up to Date

MicroPDF includes an offline, beginner-friendly step-by-step manual (`Help -> User Guide (How to Use)`) designed specifically for people with **little IT literacy**.

> **Whenever you add, remove, rename, or modify ANY user-facing feature, menu item, modal dialog, toolbar button, or keyboard/mouse shortcut, you MUST update:**
>
> 1. **[`src/viewer/user_guide.rs`](src/viewer/user_guide.rs)** — Update the matching chapter(s) (or add steps) following the exact schema, plain-English writing rules, and module-to-chapter checklist in **[`docs/USER_GUIDE_FRAMEWORK.md`](docs/USER_GUIDE_FRAMEWORK.md)**.
> 2. **`Help -> Keyboard & Mouse Shortcuts`** in **[`src/viewer/overlays.rs`](src/viewer/overlays.rs)** — Keep shortcut keycaps and descriptions 100% accurate.
> 3. **[`README.md`](README.md)** — Update feature highlights and shortcut tables when adding major capabilities.

**Timing tip:** When implementing multi-step features or large releases, update `src/viewer/user_guide.rs` **last** (after all menus, button labels, and dialogs are finalized) so every menu trail and step matches the shipped UI verbatim.

---

## 3. Pre-PR Verification Checklist

1. `cargo test` — all pure-Rust unit tests (and `test_tier*` tests, including `test_tier3_user_guide_chapters_and_export`) pass.
2. `cargo check --tests && cargo build --release --locked` — compiles with **0 errors and 0 warnings**.
3. Visually verify new/modified UI dialogs in both **Light** and **Dark** themes.
4. Confirm GitHub Actions CI passes on **both Windows (x64) and macOS (arm64)**.
