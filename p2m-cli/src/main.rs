//! CLI tool for converting PDF files to Markdown.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

/// Convert PDF files to Markdown.
#[derive(Parser)]
#[command(name = "p2m", version, about)]
struct Cli {
    /// PDF file to convert.
    file: PathBuf,

    /// Output raw markdown only (no metadata on stderr).
    #[arg(long)]
    raw: bool,

    /// Process only specific pages (1-indexed, comma-separated).
    #[arg(long, value_delimiter = ',')]
    pages: Option<Vec<u32>>,

    /// Insert page break markers between pages.
    #[arg(long)]
    page_breaks: bool,

    /// Write output to a file instead of stdout.
    #[arg(short, long)]
    output: Option<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let mut opts = p2m::Options::new();
    if let Some(pages) = cli.pages {
        opts = opts.pages(pages);
    }
    opts.markdown.include_page_breaks = cli.page_breaks;

    match p2m::convert_with(&cli.file, &opts) {
        Ok(doc) => {
            if !cli.raw {
                if let Some(title) = &doc.title {
                    eprintln!("Title: {title}");
                }
                eprintln!("Pages: {}", doc.page_count);
                eprintln!("---");
            }

            if let Some(out_path) = &cli.output {
                if let Err(e) = std::fs::write(out_path, &doc.markdown) {
                    eprintln!("error: failed to write output: {e}");
                    return ExitCode::from(1);
                }
            } else {
                print!("{}", doc.markdown);
            }

            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}
