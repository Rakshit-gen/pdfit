use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use pdfit::doc::{self, ImageInfo};
use pdfit::fit::{self, mb, parse_size};

#[derive(Parser)]
#[command(
    version,
    about = "Shrink a PDF to fit an upload limit by recompressing only its images",
    after_help = "Without --under, pdfit lists the images and what they weigh, and writes nothing.\n\
                  MB means 1,000,000 bytes, the stricter reading, so the result passes either way a site counts."
)]
struct Cli {
    /// The PDF to shrink. It is never changed.
    input: PathBuf,
    /// Size limit, like 2MB, 500KB, 1.5MB or a plain number of bytes.
    #[arg(long, value_parser = parse_size)]
    under: Option<usize>,
    /// Where to write the result. Defaults to NAME.small.pdf next to the input.
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Print a JSON report instead of text.
    #[arg(long)]
    json: bool,
}

fn default_output(input: &Path) -> PathBuf {
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".into());
    input.with_file_name(format!("{stem}.small.pdf"))
}

/// Two paths name the same file, even if one doesn't exist yet or is
/// spelled differently.
fn same_file(a: &Path, b: &Path) -> bool {
    let full = |p: &Path| {
        p.canonicalize().ok().or_else(|| {
            let parent = p
                .parent()
                .filter(|d| !d.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            Some(parent.canonicalize().ok()?.join(p.file_name()?))
        })
    };
    matches!((full(a), full(b)), (Some(x), Some(y)) if x == y)
}

fn count(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

fn image_line(i: &ImageInfo) -> String {
    let pages = match i.pages.as_slice() {
        [] => "not drawn".to_string(),
        [p] => format!("page {p}"),
        [first, .., last] => format!("pages {first}-{last}"),
    };
    let dpi = i.dpi.map_or("    ".into(), |d| format!("{d:>4.0}"));
    format!(
        "{pages:<11} {:>5} x {:<5} {:<6} {:<5} {dpi} dpi  {:>8}",
        i.width,
        i.height,
        i.format,
        i.color,
        mb(i.bytes)
    )
}

fn run(cli: Cli) -> Result<ExitCode, String> {
    let input = std::fs::read(&cli.input)
        .map_err(|e| format!("can't read {}: {e}", cli.input.display()))?;
    let name = cli.input.display();

    let Some(limit) = cli.under else {
        let doc = doc::load(&input).map_err(|e| e.to_string())?;
        let inv = doc::inventory(&doc);
        if cli.json {
            let images: Vec<&ImageInfo> = inv.images.iter().map(|i| &i.info).collect();
            let out =
                serde_json::json!({ "bytes": input.len(), "inventory": inv, "images": images });
            println!(
                "{}",
                serde_json::to_string_pretty(&out).expect("plain data")
            );
            return Ok(ExitCode::SUCCESS);
        }
        let image_bytes: usize = inv.images.iter().map(|i| i.info.bytes).sum();
        println!(
            "{name}: {}, {}, {} taking {} ({:.0}% of the file).",
            mb(input.len()),
            count(inv.pages, "page"),
            count(inv.images.len(), "image"),
            mb(image_bytes),
            100.0 * image_bytes as f64 / input.len() as f64
        );
        for i in &inv.images {
            let note = i
                .info
                .skip
                .as_deref()
                .map(|s| format!("  ({s})"))
                .unwrap_or_default();
            println!("  {}{note}", image_line(&i.info));
        }
        if inv.inline_images > 0 {
            println!(
                "  plus {} inline images, which pdfit leaves as they are",
                inv.inline_images
            );
        }
        println!("Add --under 2MB (or another size) to shrink it.");
        return Ok(ExitCode::SUCCESS);
    };

    let output = cli
        .output
        .clone()
        .unwrap_or_else(|| default_output(&cli.input));
    if same_file(&cli.input, &output) {
        return Err("the output would overwrite the input; pick another name with -o".into());
    }

    let outcome = fit::fit(&input, limit).map_err(|e| e.to_string())?;
    std::fs::write(&output, &outcome.output)
        .map_err(|e| format!("can't write {}: {e}", output.display()))?;
    let code = if outcome.fits {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(2)
    };

    if cli.json {
        let mut v = serde_json::to_value(&outcome).expect("plain data");
        v["output"] = output.display().to_string().into();
        println!("{}", serde_json::to_string_pretty(&v).expect("plain data"));
        return Ok(code);
    }

    println!(
        "{name}: {}, {}.",
        mb(input.len()),
        count(outcome.images.len(), "image")
    );
    if outcome.already_fits {
        println!(
            "It's already under {}, so {} is an unchanged copy.",
            mb(limit),
            output.display()
        );
        return Ok(code);
    }
    let tried = outcome.attempts.len();
    match (outcome.fits, outcome.setting) {
        (true, None) => println!("Lossless clean-up alone was enough."),
        (true, Some(s)) => println!(
            "Made {tried} versions; the best one that fits is {} dpi at JPEG quality {}.",
            s.dpi, s.quality
        ),
        (false, _) => println!(
            "Couldn't get it under {}. {}",
            mb(limit),
            outcome.reason.as_deref().unwrap_or("")
        ),
    }
    let saved = 100.0 * (1.0 - outcome.output_bytes as f64 / outcome.input_bytes as f64);
    println!(
        "Wrote {}: {} ({saved:.0}% smaller) in {:.1} s.",
        output.display(),
        mb(outcome.output_bytes),
        outcome.millis as f64 / 1000.0
    );
    println!("Text, fonts and drawings are byte-for-byte unchanged.");
    let changed: Vec<_> = outcome
        .images
        .iter()
        .filter(|i| i.after.is_some())
        .collect();
    if !changed.is_empty() {
        println!("Recompressed:");
        for i in changed {
            let a = i.after.as_ref().expect("filtered");
            println!(
                "  {}  -> {} x {}, {}",
                image_line(&i.info),
                a.width,
                a.height,
                mb(a.after)
            );
        }
    }
    let skipped: Vec<_> = outcome
        .images
        .iter()
        .filter(|i| i.info.skip.is_some())
        .collect();
    if !skipped.is_empty() {
        println!("Left as they are:");
        for i in skipped {
            println!(
                "  {}  ({})",
                image_line(&i.info),
                i.info.skip.as_deref().unwrap_or("")
            );
        }
    }
    Ok(code)
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("pdfit: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_output_never_lands_on_the_input() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("form.pdf");
        std::fs::write(&input, b"%PDF-1.4").unwrap();
        assert!(same_file(&input, &dir.path().join("./form.pdf")));
        assert!(!same_file(&input, &default_output(&input)));
        assert_eq!(
            default_output(&input).file_name().unwrap(),
            "form.small.pdf"
        );
    }
}
