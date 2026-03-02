use console::style;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

pub struct StepProgress {
    mp: MultiProgress,
    total_steps: u8,
    current_step: u8,
    current_bar: Option<ProgressBar>,
    current_msg: String,
}

impl StepProgress {
    pub fn new(total_steps: u8) -> Self {
        Self {
            mp: MultiProgress::new(),
            total_steps,
            current_step: 0,
            current_bar: None,
            current_msg: String::new(),
        }
    }

    pub fn start_step(&mut self, msg: &str) {
        self.complete_step("");
        self.current_step += 1;
        self.current_msg = msg.to_string();

        let pb = self.mp.add(ProgressBar::new_spinner());
        pb.set_style(
            ProgressStyle::default_spinner()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
                .template("  {spinner:.cyan} [{prefix}] {msg}")
                .unwrap(),
        );
        pb.set_prefix(format!("{}/{}", self.current_step, self.total_steps));
        pb.set_message(msg.to_string());
        pb.enable_steady_tick(std::time::Duration::from_millis(80));
        self.current_bar = Some(pb);
    }

    pub fn complete_step(&mut self, detail: &str) {
        self.finish_current(style("✓").green(), detail);
    }

    pub fn fail_step(&mut self, detail: &str) {
        self.finish_current(style("✗").red(), detail);
    }

    pub fn multi_progress(&self) -> &MultiProgress {
        &self.mp
    }

    /// Get the `MultiProgress` if `self` lives inside an `Option`.
    /// Convenience for download progress where an `Option<&MultiProgress>` is
    /// needed.
    pub fn multi_progress_opt(sp: &Option<Self>) -> Option<&MultiProgress> {
        sp.as_ref().map(|s| s.multi_progress())
    }

    fn finish_current(&mut self, icon: console::StyledObject<&str>, detail: &str) {
        if let Some(pb) = self.current_bar.take() {
            let msg = if detail.is_empty() {
                format!(
                    "{} [{}/{}] {}",
                    icon, self.current_step, self.total_steps, self.current_msg,
                )
            } else {
                format!(
                    "{} [{}/{}] {} — {}",
                    icon, self.current_step, self.total_steps, self.current_msg, detail,
                )
            };
            pb.set_style(ProgressStyle::with_template("  {msg}").unwrap());
            pb.finish_with_message(msg);
        }
    }
}

// ---------------------------------------------------------------------------
// Convenience helpers for `Option<StepProgress>` — collapse the repetitive
// `if let Some(ref mut s) = sp { s.method(...) }` pattern into one-liners.
// ---------------------------------------------------------------------------

pub fn sp_start(sp: &mut Option<StepProgress>, msg: &str) {
    if let Some(s) = sp.as_mut() {
        s.start_step(msg);
    }
}

pub fn sp_complete(sp: &mut Option<StepProgress>, detail: &str) {
    if let Some(s) = sp.as_mut() {
        s.complete_step(detail);
    }
}

pub fn sp_fail(sp: &mut Option<StepProgress>, detail: &str) {
    if let Some(s) = sp.as_mut() {
        s.fail_step(detail);
    }
}
