use std::process;

use clap::{Parser, ValueEnum};
use sitegrab::crawler;
use sitegrab::manifest;
use sitegrab::offline;
use url::Url;

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RenderMode {
    /// Detect whether the site is a SPA and render only if needed
    Auto,
    /// Force headless-browser rendering for every page
    On,
    /// Plain HTTP crawling only
    Off,
}

#[derive(Parser)]
#[command(
    name = "sitegrab",
    version,
    long_about = "\
sitegrab — Download a website for offline browsing.

Simple one-command mirroring:
  sitegrab https://example.com

Automatically detects SPA (Vue/React/Angular) sites and renders them
with a headless browser so the content is fully captured.

Supports incremental updates — re-running mirrors only new/changed files.
Use --robots to obey robots.txt, --fresh for a full re-download."
)]
struct Args {
    /// URL of the website to mirror
    url: String,

    /// Output directory (defaults to domain name)
    #[arg(short, long)]
    output: Option<String>,

    /// Number of concurrent downloads (must be >= 1)
    #[arg(short, long, default_value = "8", value_parser = clap::value_parser!(u16).range(1..))]
    jobs: u16,

    /// Skip ZIP archive creation
    #[arg(long)]
    no_zip: bool,

    /// Force fresh download (ignore existing manifest)
    #[arg(long)]
    fresh: bool,

    /// Respect robots.txt (default: no)
    #[arg(long)]
    robots: bool,

    /// SPA rendering mode: auto (default), on, or off.
    /// "auto" detects whether the site is a SPA and renders if needed.
    /// "on" forces headless-browser rendering for every page.
    /// "off" uses plain HTTP crawling only.
    #[arg(long, default_value = "auto", value_enum)]
    render: RenderMode,

    /// Settle time (ms) to wait after page load for lazy/AJAX content.
    /// Only relevant when rendering is active. Default: 1500
    #[arg(long, default_value = "1500")]
    wait: u64,

    /// Maximum number of HTML pages to download (default: 10000)
    #[arg(long, default_value = "10000")]
    max_pages: usize,

    /// Maximum total downloaded bytes (0 = unlimited)
    #[arg(long, default_value = "0")]
    max_bytes: u64,

    /// Disable Chromium sandbox (needed in some containers; less secure)
    #[arg(long)]
    no_sandbox: bool,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let url = match Url::parse(&args.url) {
        Ok(u) => {
            if u.scheme() != "http" && u.scheme() != "https" {
                eprintln!("error: URL must start with http:// or https://");
                process::exit(1);
            }
            u
        }
        Err(e) => {
            eprintln!("error: Invalid URL '{}': {}", args.url, e);
            process::exit(1);
        }
    };

    let host = match url.host_str() {
        Some(h) => h.to_string(),
        None => {
            eprintln!("error: URL must have a host (e.g. https://example.com)");
            process::exit(1);
        }
    };

    let output_dir = args.output.unwrap_or_else(|| host.clone());

    // ── Determine crawl mode ──────────────────────────────────────────
    // If the user didn't explicitly choose, fetch the first page and
    // analyse it to decide whether headless-browser rendering is needed.
    let use_render = match args.render {
        RenderMode::On => true,
        RenderMode::Off => false,
        RenderMode::Auto => {
            // Auto-detect
            eprintln!("info: Detecting site type...");
            let is_spa = crawler::detect_spa(&url).await;
            if is_spa {
                eprintln!("info: SPA detected (React/Vue/Angular) — switching to render mode");
            } else {
                eprintln!("info: Static site detected — plain HTTP crawl");
            }
            is_spa
        }
    };

    // Verify render support is compiled in
    #[cfg(not(feature = "render"))]
    if use_render {
        eprintln!("error: SPA rendering is needed but this binary was built without the `render` feature.");
        eprintln!("       Rebuild with: cargo build --features render");
        process::exit(1);
    }

    // Load or create manifest
    let (manifest, loaded_existing_manifest) = if args.fresh {
        let _ = std::fs::create_dir_all(&output_dir);
        let mut mf = manifest::Manifest::new(url.as_str());
        let _ = mf.save_to(&output_dir);
        eprintln!("info: Fresh download, created new manifest");
        (Some(tokio::sync::Mutex::new(mf)), false)
    } else {
        match manifest::Manifest::load_from(&output_dir) {
            Ok(Some(mf)) => {
                eprintln!("info: Found existing manifest — incremental mode");
                (Some(tokio::sync::Mutex::new(mf)), true)
            }
            Ok(None) => {
                let _ = std::fs::create_dir_all(&output_dir);
                (
                    Some(tokio::sync::Mutex::new(manifest::Manifest::new(
                        url.as_str(),
                    ))),
                    false,
                )
            }
            Err(e) => {
                eprintln!("warning: Failed to load manifest: {e}, starting fresh");
                let _ = std::fs::create_dir_all(&output_dir);
                (
                    Some(tokio::sync::Mutex::new(manifest::Manifest::new(
                        url.as_str(),
                    ))),
                    false,
                )
            }
        }
    };

    println!("sitegrab v{}", env!("CARGO_PKG_VERSION"));
    println!("Mirroring: {}", url);
    println!("Output:    {}/", output_dir);
    println!("Workers:   {}", args.jobs);
    if use_render {
        println!("Mode:      SPA render (headless browser)");
        if args.no_sandbox {
            println!("           Chromium --no-sandbox enabled");
        }
    } else {
        println!("Mode:      plain HTTP crawl");
    }
    if !args.fresh && loaded_existing_manifest {
        println!("           incremental (use --fresh for full re-download)");
    }
    println!();

    let limits = crawler::CrawlLimits {
        max_pages: args.max_pages,
        max_bytes: args.max_bytes,
    };

    let crawl_result = if use_render {
        #[cfg(feature = "render")]
        {
            crawler::crawl_spa(
                &url,
                &output_dir,
                args.jobs as usize,
                manifest,
                args.robots,
                args.wait,
                limits,
                args.no_sandbox,
            )
            .await
        }
        #[cfg(not(feature = "render"))]
        {
            // Unreachable — guarded above
            Err(anyhow::anyhow!("render feature not enabled"))
        }
    } else {
        crawler::crawl(
            &url,
            &output_dir,
            args.jobs as usize,
            manifest,
            args.robots,
            limits,
        )
        .await
    };

    match crawl_result {
        Ok(stats) => {
            if !args.no_zip {
                let zip_path = format!("{}.zip", output_dir);
                if let Err(e) = sitegrab::archiver::create_zip(&output_dir, &zip_path) {
                    eprintln!("warning: Failed to create zip: {e}");
                } else if let Err(e) = offline::assert_offline_closure(&output_dir, &host) {
                    eprintln!("warning: Offline closure check failed: {e}");
                }
            } else if let Err(e) = offline::assert_offline_closure(&output_dir, &host) {
                eprintln!("warning: Offline closure check failed: {e}");
            }

            match stats.outcome {
                crawler::CrawlOutcome::Complete => {}
                crawler::CrawlOutcome::Partial => {
                    if stats.errors > 0 {
                        eprintln!("warning: {} resource errors during crawl", stats.errors);
                    }
                    process::exit(2);
                }
                crawler::CrawlOutcome::Failed => {
                    process::exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!("error: Crawl failed: {e}");
            process::exit(1);
        }
    }
}
