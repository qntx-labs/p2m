//! CLI tool for converting PDF files to Markdown.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 || args[1] == "--help" || args[1] == "-h" {
        eprintln!("Usage: p2m [OPTIONS] <PDF_FILE>");
        eprintln!();
        eprintln!("Options:");
        eprintln!("  --raw         Output raw markdown only (no metadata)");
        eprintln!("  --pages N,M   Process only specific pages (1-indexed, comma-separated)");
        eprintln!("  -h, --help    Show this help message");
        return if args.len() < 2 {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        };
    }

    let mut path: Option<&str> = None;
    let mut raw = false;
    let mut page_list: Option<Vec<u32>> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--raw" => raw = true,
            "--pages" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("error: --pages requires a value");
                    return ExitCode::from(1);
                }
                let pages: Result<Vec<u32>, _> =
                    args[i].split(',').map(|s| s.trim().parse::<u32>()).collect();
                match pages {
                    Ok(p) => page_list = Some(p),
                    Err(e) => {
                        eprintln!("error: invalid page number: {e}");
                        return ExitCode::from(1);
                    }
                }
            }
            arg if arg.starts_with('-') => {
                eprintln!("error: unknown option: {arg}");
                return ExitCode::from(1);
            }
            _ => path = Some(&args[i]),
        }
        i += 1;
    }

    let Some(pdf_path) = path else {
        eprintln!("error: no PDF file specified");
        return ExitCode::from(1);
    };

    let mut opts = p2m::Options::new();
    if let Some(pages) = page_list {
        opts = opts.pages(pages);
    }

    match p2m::convert_with(pdf_path, &opts) {
        Ok(doc) => {
            if !raw {
                if let Some(title) = &doc.title {
                    eprintln!("Title: {title}");
                }
                eprintln!("Pages: {}", doc.page_count);
                eprintln!("---");
            }
            print!("{}", doc.markdown);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}
