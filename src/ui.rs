//! Terminal UI: banner, spinner steps, prompts. `--yes` answers every prompt with its default.
use anyhow::{bail, Result};
use console::{style, Term};
use dialoguer::{theme::ColorfulTheme, Confirm, Input, MultiSelect, Select};
use indicatif::{ProgressBar, ProgressStyle};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static YES: AtomicBool = AtomicBool::new(false);

pub fn set_yes(v: bool) {
    YES.store(v, Ordering::Relaxed);
}

fn yes() -> bool {
    YES.load(Ordering::Relaxed)
}

pub fn banner() {
    println!(
        "\n  {}  {}\n  {}\n",
        style("▌wrtforge").magenta().bold(),
        style(format!("v{}", env!("CARGO_PKG_VERSION"))).dim(),
        style("clean official OpenWrt + Passwall2, over SSH").dim()
    );
}

pub fn head(s: &str) {
    println!("\n  {}", style(s).bold());
}

pub fn info(s: &str) {
    println!("    {}", style(s).dim());
}

pub fn warn(s: &str) {
    println!("  {} {}", style("!").yellow(), s);
}

pub fn ok(s: &str) {
    println!("  {} {}", style("✔").green(), s);
}

/// A spinner line. Finish it with `done`/`warn`; dropping it unfinished prints a failure mark.
pub struct Step {
    pb: ProgressBar,
    msg: String,
    finished: bool,
}

impl Step {
    pub fn new(msg: &str) -> Step {
        let pb = ProgressBar::new_spinner();
        pb.set_style(ProgressStyle::with_template("  {spinner:.cyan} {msg}").unwrap());
        pb.enable_steady_tick(Duration::from_millis(80));
        pb.set_message(msg.to_string());
        if !Term::stdout().is_term() {
            println!("  … {msg}");
        }
        Step {
            pb,
            msg: msg.to_string(),
            finished: false,
        }
    }

    fn end(mut self, mark: console::StyledObject<&str>, msg: &str) {
        self.finished = true;
        self.pb.finish_and_clear();
        println!("  {} {}", mark, msg);
    }

    pub fn done(self, msg: &str) {
        self.end(style("✔").green(), msg)
    }

    pub fn warn(self, msg: &str) {
        self.end(style("!").yellow(), msg)
    }
}

impl Drop for Step {
    fn drop(&mut self) {
        if !self.finished {
            self.pb.finish_and_clear();
            println!("  {} {}", style("✖").red(), self.msg);
        }
    }
}

pub fn ask(q: &str, default: &str) -> Result<String> {
    if yes() {
        return Ok(default.to_string());
    }
    Ok(Input::with_theme(&ColorfulTheme::default())
        .with_prompt(q)
        .default(default.to_string())
        .interact_text()?)
}

pub fn confirm(q: &str, default: bool) -> Result<bool> {
    if yes() {
        return Ok(default);
    }
    Ok(Confirm::with_theme(&ColorfulTheme::default())
        .with_prompt(q)
        .default(default)
        .interact()?)
}

pub fn choose(q: &str, items: &[String], default: usize) -> Result<usize> {
    if yes() {
        return Ok(default);
    }
    if items.is_empty() {
        bail!("nothing to choose from");
    }
    Ok(Select::with_theme(&ColorfulTheme::default())
        .with_prompt(q)
        .items(items)
        .default(default)
        .interact()?)
}

/// Multi-select; returns the chosen indexes. With `--yes` it returns the pre-ticked ones.
pub fn multi(q: &str, items: &[String], defaults: &[bool]) -> Result<Vec<usize>> {
    if yes() {
        return Ok(defaults
            .iter()
            .enumerate()
            .filter(|(_, d)| **d)
            .map(|(i, _)| i)
            .collect());
    }
    Ok(MultiSelect::with_theme(&ColorfulTheme::default())
        .with_prompt(q)
        .items(items)
        .defaults(defaults)
        .interact()?)
}
