#[macro_use]
pub mod fmt;
pub mod background;
pub mod notification;
pub mod term;

use std::fmt::Write as _;
use std::fmt::{Debug, Display};
use std::io::Write;
use std::time::Duration;

use crate::ui::output::OutputFormat;
use crate::{Result, UiError};

use colorful::Colorful;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use miette::WrapErr;
use miette::{miette, IntoDiagnostic};
use ockam_core::env::get_env_with_default;

use r3bl_rs_utils_core::*;
use r3bl_tuify::*;
use tokio::sync::Mutex;
use tokio::time::sleep;
use tracing::warn;

/// A terminal abstraction to handle commands' output and messages styling.
#[derive(Clone, Debug)]
pub struct Terminal<T: TerminalWriter + Debug, WriteMode = ToStdErr> {
    stdout: T,
    stderr: T,
    quiet: bool,
    no_input: bool,
    output_format: OutputFormat,
    mode: WriteMode,
    max_width_col_count: usize,
    max_height_row_count: usize,
}

impl<T: TerminalWriter + Debug, W> Terminal<T, W> {
    pub fn is_quiet(&self) -> bool {
        self.quiet
    }
}

/// A small wrapper around the `Write` trait, enriched with CLI
/// attributes to facilitate output handling.
#[derive(Clone, Debug)]
pub struct TerminalStream<T: Write + Debug + Clone> {
    pub writer: T,
    pub no_color: bool,
}

impl<T: Write + Debug + Clone> TerminalStream<T> {
    pub fn prepare_msg(&self, msg: impl AsRef<str>) -> Result<String> {
        let mut buffer = Vec::new();
        write!(buffer, "{}", msg.as_ref())?;
        if self.no_color {
            buffer = strip_ansi_escapes::strip(&buffer);
        }
        Ok(String::from_utf8(buffer)
            .into_diagnostic()
            .wrap_err("Invalid UTF-8")?)
    }
}

/// Trait defining the main methods to write messages to a terminal stream.
pub trait TerminalWriter: Clone {
    fn stdout(no_color: bool) -> Self;
    fn stderr(no_color: bool) -> Self;
    fn is_tty(&self) -> bool;

    fn write(&mut self, s: impl AsRef<str>) -> Result<()>;
    fn rewrite(&mut self, s: impl AsRef<str>) -> Result<()>;
    fn write_line(&self, s: impl AsRef<str>) -> Result<()>;
}

// Core functions
impl<W: TerminalWriter + Debug> Terminal<W> {
    pub fn new(quiet: bool, no_color: bool, no_input: bool, output_format: OutputFormat) -> Self {
        let no_color = Self::should_disable_color(no_color);
        let no_input = Self::should_disable_user_input(no_input);
        let stdout = W::stdout(no_color);
        let stderr = W::stderr(no_color);
        let max_width_col_count = get_size().map(|it| it.col_count).unwrap_or(ch!(80)).into();
        Self {
            stdout,
            stderr,
            quiet,
            no_input,
            output_format,
            mode: ToStdErr,
            max_width_col_count,
            max_height_row_count: 5,
        }
    }

    pub fn is_tty(&self) -> bool {
        self.stderr.is_tty()
    }

    pub fn quiet() -> Self {
        Self::new(true, false, false, OutputFormat::Plain)
    }

    /// Prompt the user for a confirmation.
    pub fn confirm(&self, msg: impl AsRef<str>) -> Result<ConfirmResult> {
        if !self.can_ask_for_user_input() {
            return Ok(ConfirmResult::NonTTY);
        }
        Ok(ConfirmResult::from(
            dialoguer::Confirm::new()
                .default(true)
                .show_default(true)
                .with_prompt(fmt_warn!("{}", msg.as_ref()))
                .interact()
                .map_err(UiError::Dialoguer)?,
        ))
    }

    pub fn confirmed_with_flag_or_prompt(
        &self,
        flag: bool,
        prompt_msg: impl AsRef<str>,
    ) -> Result<bool> {
        if flag {
            Ok(true)
        } else {
            // If the confirmation flag is not provided, prompt the user.
            match self.confirm(prompt_msg)? {
                ConfirmResult::Yes => Ok(true),
                ConfirmResult::No => Ok(false),
                ConfirmResult::NonTTY => Err(miette!("Use --yes to confirm"))?,
            }
        }
    }

    pub fn confirm_interactively(&self, header: String) -> bool {
        let user_input = select_from_list(
            header,
            ["YES", "NO"].iter().map(|it| it.to_string()).collect(),
            self.max_height_row_count,
            self.max_width_col_count,
            SelectionMode::Single,
            StyleSheet::default(),
        );

        match &user_input {
            Some(it) => it.contains(&"YES".to_string()),
            None => false,
        }
    }

    /// Returns the selected items by the user, or an empty `Vec` if the user did not select any item
    /// or if the user is not able to select an item (e.g. not a TTY, `--no-input` flag, etc.).
    pub fn select_multiple(&self, header: String, items: Vec<String>) -> Vec<String> {
        if !self.can_ask_for_user_input() {
            return Vec::new();
        }

        let user_selected_list = select_from_list(
            header,
            items,
            self.max_height_row_count,
            self.max_width_col_count,
            SelectionMode::Multiple,
            StyleSheet::default(),
        );

        user_selected_list.unwrap_or_default()
    }

    pub fn can_ask_for_user_input(&self) -> bool {
        !self.no_input && self.stderr.is_tty() && !self.quiet
    }

    fn should_disable_color(no_color: bool) -> bool {
        // If global argument `--no-color` is passed or the `NO_COLOR` env var is set, colors
        // will be stripped out from output messages. Otherwise, let the terminal decide.
        no_color || get_env_with_default("NO_COLOR", false).unwrap_or(false)
    }

    fn should_disable_user_input(no_input: bool) -> bool {
        // If global argument `--no-input` is passed or the `NO_INPUT` env var is set we won't be able
        // to ask the user for input.  Otherwise, let the terminal decide based on the `is_tty` value
        no_input || get_env_with_default("NO_INPUT", false).unwrap_or(false)
    }

    pub fn set_quiet(&self) -> Self {
        let mut clone = self.clone();
        clone.quiet = true;
        clone
    }
}

// Logging mode
impl<W: TerminalWriter + Debug> Terminal<W, ToStdErr> {
    pub fn write(&self, msg: impl AsRef<str>) -> Result<()> {
        if self.quiet {
            return Ok(());
        }
        self.stderr.clone().write(msg)
    }

    pub fn rewrite(&self, msg: impl AsRef<str>) -> Result<()> {
        if self.quiet {
            return Ok(());
        }
        self.stderr.clone().rewrite(msg)
    }

    pub fn write_line(&self, msg: impl AsRef<str>) -> Result<&Self> {
        if self.quiet {
            return Ok(self);
        }
        self.stderr
            .write_line(msg)
            .map_err(|e| miette!("Unable to write to stderr, {e}"))?;
        Ok(self)
    }

    pub fn build_list(
        &self,
        items: &[impl crate::output::Output],
        header: &str,
        empty_message: &str,
    ) -> Result<String> {
        let mut output = String::new();

        // Early return, if the items are empty.
        if items.is_empty() {
            let empty_message = fmt_info!("{}", empty_message);
            write!(output, "{}", empty_message)?;
            return Ok(output);
        }

        // Display header
        let header_display_width = {
            // Strip ANSI escape codes from header to correctly calculate its display width
            let header_stripped = strip_ansi_escapes::strip(header);
            header_stripped.len()
        };
        let padding = 7;
        writeln!(
            output,
            "{}",
            &fmt_log!("┌{}┐", "─".repeat(header_display_width + (padding * 2)))
        )?;
        writeln!(
            output,
            "{}",
            &fmt_log!("│{}{header}{}│", " ".repeat(padding), " ".repeat(padding))
        )?;
        writeln!(
            output,
            "{}",
            &fmt_log!("└{}┘\n", "─".repeat(header_display_width + (padding * 2)))
        )?;

        // Display items with alternating colors
        for item in items {
            let item = item.list()?;
            item.split('\n').for_each(|line| {
                let _ = writeln!(output, "{}", &fmt_list!("{line}"));
            });
            writeln!(output)?;
        }

        Ok(output)
    }

    pub fn stdout(self) -> Terminal<W, ToStdOut> {
        Terminal {
            stdout: self.stdout,
            stderr: self.stderr,
            quiet: self.quiet,
            no_input: self.no_input,
            output_format: self.output_format,
            mode: ToStdOut {
                output: Output::new(),
            },
            max_width_col_count: self.max_width_col_count,
            max_height_row_count: self.max_height_row_count,
        }
    }
}

// Finished mode
impl<W: TerminalWriter + Debug> Terminal<W, ToStdOut> {
    pub fn is_tty(&self) -> bool {
        self.stdout.is_tty()
    }

    pub fn plain<T: Display>(mut self, msg: T) -> Self {
        self.mode.output.plain = Some(msg.to_string());
        self
    }

    pub fn machine<T: Display>(mut self, msg: T) -> Self {
        self.mode.output.machine = Some(msg.to_string());
        self
    }

    pub fn json<T: Display>(mut self, msg: T) -> Self {
        self.mode.output.json = Some(msg.to_string());
        self
    }

    pub fn write_line(self) -> Result<()> {
        // Check that there is at least one output format defined
        if self.mode.output.plain.is_none()
            && self.mode.output.machine.is_none()
            && self.mode.output.json.is_none()
        {
            return Err(miette!("At least one output format must be defined"))?;
        }

        let plain = self.mode.output.plain.as_ref();
        let machine = self.mode.output.machine.as_ref();
        let json = self.mode.output.json.as_ref();

        let msg = match self.output_format {
            OutputFormat::Plain => {
                // If interactive, use the following priority: Plain -> Machine -> JSON
                if self.stdout.is_tty() {
                    match (plain, machine, json) {
                        (Some(plain), _, _) => plain,
                        (None, Some(machine), _) => machine,
                        (None, None, Some(json)) => json,
                        _ => unreachable!(),
                    }
                }
                // If not interactive, use the following priority: Machine -> JSON -> Plain
                else {
                    match (machine, json, plain) {
                        (Some(machine), _, _) => machine,
                        (None, Some(json), _) => json,
                        (None, None, Some(plain)) => plain,
                        _ => unreachable!(),
                    }
                }
            }
            OutputFormat::Json => match json {
                Some(json) => json,
                // If not set, no fallback is provided
                None => {
                    warn!("JSON output is not defined for this command");
                    return Ok(());
                }
            },
        };
        self.stdout.write_line(msg)
    }
}

// Extensions
impl<W: TerminalWriter + Debug> Terminal<W> {
    pub fn can_use_progress_spinner(&self) -> bool {
        !self.quiet && self.stderr.is_tty()
    }

    pub fn progress_spinner(&self) -> Option<ProgressBar> {
        if !self.can_use_progress_spinner() {
            return None;
        }
        let ticker = [
            "     ⠋", "     ⠙", "     ⠹", "     ⠸", "     ⠼", "     ⠴", "     ⠦", "     ⠧",
            "     ⠇", "     ⠏",
        ];

        let pb = ProgressBar::new_spinner();
        pb.set_draw_target(ProgressDrawTarget::stderr());
        pb.enable_steady_tick(Duration::from_millis(80));
        pb.set_style(
            ProgressStyle::with_template("{spinner:.yellow} {msg}")
                .expect("Failed to set progress bar template")
                .tick_strings(&ticker),
        );
        Some(pb)
    }

    pub async fn progress_output(
        &self,
        output_messages: &[String],
        is_finished: &Mutex<bool>,
    ) -> miette::Result<()> {
        if output_messages.is_empty() {
            return Ok(());
        }
        let spinner = self.progress_spinner();
        self.progress_output_with_progress_bar(output_messages, is_finished, spinner.as_ref())
            .await
    }

    pub async fn progress_output_with_progress_bar(
        &self,
        output_messages: &[String],
        is_finished: &Mutex<bool>,
        progress_bar: Option<&ProgressBar>,
    ) -> miette::Result<()> {
        let progress_bar = match progress_bar {
            Some(pb) => pb,
            None => return Ok(()),
        };

        loop {
            if *is_finished.lock().await {
                progress_bar.finish_and_clear();
                break;
            }

            for message in output_messages {
                progress_bar.set_message(message.clone());
                sleep(Duration::from_millis(500)).await;
                if *is_finished.lock().await {
                    progress_bar.finish_and_clear();
                    break;
                }
            }
        }

        Ok(())
    }
}

/// Write mode used when writing to the stderr stream.
#[derive(Clone, Debug)]
pub struct ToStdErr;

/// Write mode used when writing to the stdout stream.
#[derive(Clone, Debug)]
pub struct ToStdOut {
    pub(self) output: Output,
}

/// The command's output message to be displayed to the user in various formats
#[derive(Clone, Debug)]
struct Output {
    plain: Option<String>,
    machine: Option<String>,
    json: Option<String>,
}

impl Output {
    fn new() -> Self {
        Self {
            plain: None,
            machine: None,
            json: None,
        }
    }
}

pub enum ConfirmResult {
    Yes,
    No,
    NonTTY,
}

impl From<bool> for ConfirmResult {
    fn from(value: bool) -> Self {
        if value {
            ConfirmResult::Yes
        } else {
            ConfirmResult::No
        }
    }
}
