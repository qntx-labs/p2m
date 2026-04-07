<!-- markdownlint-disable MD033 MD041 MD036 -->

# P2M

Fast, pure-Rust PDF to Markdown converter. No ML, no OCR, no external dependencies.

## Features

- **Text extraction** — CMap/ToUnicode decoding, CID fonts, TrueType fallback, ligature expansion
- **Layout analysis** — multi-column detection (newspaper & tabular), reading-order reconstruction
- **Table detection** — rect-based (union-find clustering) and line-based (H/V grid intersection)
- **Markdown generation** — headings, lists, code blocks, bold/italic, hyperlinks, page breaks
- **Tagged PDF support** — structure-tree roles (H1–H6, P, L, Code, BlockQuote, Table)

## Installation

```sh
cargo install p2m-cli
```

Or build from source:

```sh
git clone https://github.com/qntx-labs/p2m
cd p2m
cargo build --release
```

## CLI Usage

```sh
p2m document.pdf                     # convert to stdout
p2m document.pdf -o output.md        # write to file
p2m document.pdf --pages 1,3,5       # specific pages only
p2m document.pdf --page-breaks       # insert page markers
p2m document.pdf --raw               # no metadata on stderr
```

## Library Usage

```rust
// Simple conversion
let doc = p2m::convert("document.pdf")?;
println!("{}", doc.markdown);

// With options
let opts = p2m::Options::new().pages([1, 2, 3]);
let doc = p2m::convert_with("document.pdf", &opts)?;

// From bytes
let bytes = std::fs::read("document.pdf")?;
let doc = p2m::convert_bytes(&bytes)?;
```

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT License ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project shall be dual-licensed as above, without any additional terms or conditions.

---

<div align="center">

A **[QNTX](https://qntx.fun)** open-source project.

<a href="https://qntx.fun"><img alt="QNTX" width="369" src="https://raw.githubusercontent.com/qntx/.github/main/profile/qntx-banner.svg" /></a>

<!--prettier-ignore-->
Code is law. We write both.

</div>
