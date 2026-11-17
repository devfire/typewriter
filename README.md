# Typewriter PDF

A small desktop app for filling in and signing PDFs. Open a file, type text or place a signature on a page, and save.

## Build and run

You need Rust. The toolchain version is pinned in `rust-toolchain.toml`, so rustup will fetch it on first build.

On Debian or Ubuntu, install the windowing libraries first:

```sh
sudo apt-get install libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libssl-dev
```

Then:

```sh
cargo run --release -- path/to/form.pdf
```

The PDF argument is optional. Without it, the app starts empty and you can open a file from the window.

```
typewriter [OPTIONS] [PDF]

  -h, --help     Show help
  -V, --version  Show version
```

## Using it

1. Open a PDF with Ctrl+O or the Open button.
2. Pick Type or Sign mode, then click the page where you want the box.
3. Drag boxes to move them. Drag the corner handle to resize.
4. Save with Ctrl+S, or use Save As to write to another file.

Other shortcuts:

| Action | Keys |
| --- | --- |
| Copy, cut, paste | Ctrl+C, Ctrl+X, Ctrl+V |
| Duplicate | Ctrl+D |
| Undo, redo | Ctrl+Z, Ctrl+Shift+Z |
| Delete selected box | Delete or Backspace |
| Deselect, back to Navigate mode | Escape |
| Zoom | + and - |

Signature boxes use the bundled Momo Signature script font. Text boxes use Helvetica metrics, so the on-screen preview matches the saved file.

## How saving works

Saves are incremental. The original bytes are kept as they are, and only the new page content and its fonts are appended, followed by a new cross-reference table. This means:

- Byte ranges covered by existing digital signatures are not touched.
- Boxes are stored in the PDF itself, so reopening the file in Typewriter brings them back as editable boxes.

Saving writes to a temporary file and renames it over the target, so a failed write does not leave a half-written PDF behind. Saving over the file you opened asks for confirmation the first time.

Encrypted PDFs are refused, including files that only have an owner password. Only one PDF is open at a time.

## Development

```sh
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings -D clippy::pedantic
```

CI runs the same three checks. Source modules are kept under 500 lines each.

## Fonts

`assets/LiberationSans-Regular.ttf` and `assets/MomoSignature-Regular.ttf` are licensed under the SIL Open Font License 1.1. The license texts are in `assets/`.
