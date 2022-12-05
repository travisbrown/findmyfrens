use chrono::Utc;
use clap::Parser;
use log::LevelFilter;
use reqwest::Url;
use scraper::{ElementRef, Html, Selector};
use std::path::Path;

const BASE_URL: &str = "https://findmyfrens.net/";
const SNAPSHOT_BASE_DIR: &str = "snapshot";
const TIMESTAMP_FMT: &str = "%Y%m%d%H%M%S";

#[tokio::main]
async fn main() -> Result<(), Error> {
    let opts: Opts = Opts::parse();
    let _ = init_logging(opts.verbose);

    let base_url = Url::parse(opts.base.as_deref().unwrap_or(BASE_URL))?;
    let snapshot_dir = if opts.disable_snapshot {
        None
    } else {
        Some(Path::new(SNAPSHOT_BASE_DIR).join(Utc::now().format(TIMESTAMP_FMT).to_string()))
    };

    let index = get_html(&base_url, snapshot_dir.as_ref()).await?;
    let users = index
        .select(&BODY_LIST_SEL)
        .map(parse_a)
        .collect::<Result<Vec<_>, _>>()?;

    log::info!("Downloading {} users", users.len());

    let mut writer = csv::WriterBuilder::new().from_writer(std::io::stdout());

    for (raw_url, display_name) in users {
        let user_url = base_url.join(&raw_url)?;
        let screen_name = raw_url
            .trim_end_matches('/')
            .split('/')
            .last()
            .ok_or_else(|| Error::InvalidHtml("Missing screen name".to_string()))?;

        for (url, title) in get_user(
            &user_url,
            snapshot_dir.as_ref().map(|dir| dir.join(screen_name)),
            screen_name,
            &display_name,
        )
        .await?
        {
            writer.write_record(&[screen_name, &display_name, &title, &url])?;
        }
    }

    Ok(())
}

async fn get_user<P: AsRef<Path>>(
    url: &Url,
    snapshot_dir: Option<P>,
    screen_name: &str,
    display_name: &str,
) -> Result<Vec<(String, String)>, Error> {
    log::info!("Downloading {} ({})", screen_name, display_name);
    let doc = get_html(url, snapshot_dir).await?;

    if let Some(h1) = doc.select(&BODY_MAIN_H1).collect::<Vec<_>>().first() {
        let h1_text = h1.inner_html();
        if h1_text.trim() != display_name {
            log::warn!(
                "Expected \"{}\", found \"{}\"",
                display_name,
                h1_text.trim()
            );
        }
    }

    doc.select(&BODY_MAIN_LIST_SEL).map(parse_a).collect()
}

async fn get_html<P: AsRef<Path>>(url: &Url, snapshot_dir: Option<P>) -> Result<Html, Error> {
    let response = reqwest::get(url.clone()).await?;
    let text = response.text().await?;
    let doc = Html::parse_document(&text);

    if let Some(snapshot_dir) = snapshot_dir {
        std::fs::create_dir_all(&snapshot_dir)?;
        std::fs::write(snapshot_dir.as_ref().join("index.html"), text)?;
        if let Some((stylesheet_url, stylesheet_filename)) = get_stylesheet(&doc, url)? {
            save_file(
                stylesheet_url,
                snapshot_dir.as_ref().join(stylesheet_filename),
            )
            .await?;
        }
        if let Some((banner_url, banner_filename)) = get_img(&doc, url, &BANNER_IMG_SEL)? {
            save_file(banner_url, snapshot_dir.as_ref().join(banner_filename)).await?;
        }
        if let Some((profile_url, profile_filename)) = get_img(&doc, url, &PROFILE_IMG_SEL)? {
            save_file(profile_url, snapshot_dir.as_ref().join(profile_filename)).await?;
        }
    }

    Ok(doc)
}

async fn save_file<P: AsRef<Path>>(url: Url, path: P) -> Result<(), Error> {
    let response = reqwest::get(url).await?;
    let bytes = response.bytes().await?;
    std::fs::write(path, bytes)?;

    Ok(())
}

fn get_stylesheet(doc: &Html, base_url: &Url) -> Result<Option<(Url, String)>, Error> {
    if let Some(link) = doc.select(&STYLESHEET_SEL).next() {
        let href = link
            .value()
            .attr("href")
            .ok_or_else(|| Error::InvalidHtml("Missing href for stylesheet link".to_string()))?;
        let filename = href
            .split('/')
            .last()
            .ok_or_else(|| Error::InvalidHtml("Invalid href for stylesheet link".to_string()))?;
        Ok(Some((base_url.join(href)?, filename.to_string())))
    } else {
        Ok(None)
    }
}

fn get_img(
    doc: &Html,
    base_url: &Url,
    selector: &Selector,
) -> Result<Option<(Url, String)>, Error> {
    if let Some(link) = doc.select(selector).next() {
        let src = link
            .value()
            .attr("src")
            .ok_or_else(|| Error::InvalidHtml("Missing src for img".to_string()))?;
        let filename = src
            .split('/')
            .last()
            .ok_or_else(|| Error::InvalidHtml("Invalid src for img".to_string()))?;
        Ok(Some((base_url.join(src)?, filename.to_string())))
    } else {
        Ok(None)
    }
}

fn parse_a(element: ElementRef) -> Result<(String, String), Error> {
    let href = element
        .value()
        .attr("href")
        .ok_or_else(|| Error::InvalidHtml("Invalid href for link".to_string()))?;
    let content = element.inner_html();

    Ok((href.to_string(), content))
}

lazy_static::lazy_static! {
    static ref STYLESHEET_SEL: Selector = Selector::parse("head > link[rel='stylesheet']").unwrap();
    static ref BODY_LIST_SEL: Selector = Selector::parse("body > a").unwrap();
    static ref BODY_MAIN_LIST_SEL: Selector = Selector::parse("body > main > a").unwrap();
    static ref BANNER_IMG_SEL: Selector = Selector::parse("body > header > img").unwrap();
    static ref PROFILE_IMG_SEL: Selector = Selector::parse("body > main > img").unwrap();
    static ref BODY_MAIN_H1: Selector = Selector::parse("body > main > h1").unwrap();
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Logging initialization error")]
    LogInit(#[from] log::SetLoggerError),
    #[error("I/O error")]
    Io(#[from] std::io::Error),
    #[error("HTTP client error")]
    HttpClient(#[from] reqwest::Error),
    #[error("URL error")]
    Url(#[from] url::ParseError),
    #[error("CDV error")]
    Csv(#[from] csv::Error),
    #[error("Invalid HTML")]
    InvalidHtml(String),
}

#[derive(Parser)]
#[clap(name = "scrape", version, author)]
struct Opts {
    /// Level of verbosity
    #[clap(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,
    /// The base URL
    #[clap(long)]
    base: Option<String>,
    /// Disable local copy
    #[clap(long)]
    disable_snapshot: bool,
}

fn select_log_level_filter(verbosity: u8) -> LevelFilter {
    match verbosity {
        0 => LevelFilter::Off,
        1 => LevelFilter::Error,
        2 => LevelFilter::Warn,
        3 => LevelFilter::Info,
        4 => LevelFilter::Debug,
        _ => LevelFilter::Trace,
    }
}

fn init_logging(verbosity: u8) -> Result<(), log::SetLoggerError> {
    simplelog::TermLogger::init(
        select_log_level_filter(verbosity),
        simplelog::Config::default(),
        simplelog::TerminalMode::Stderr,
        simplelog::ColorChoice::Auto,
    )
}
