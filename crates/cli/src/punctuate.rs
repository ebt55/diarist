//! `diarist-cli punctuate <meeting> | --all-missing [--remove]` (plan 16
//! section 3.9): adds punctuation to a finished meeting as a new current
//! run, with the selected punctuation model of the data root (and the
//! `--models-dir` / `--engine-dir` overrides); `--remove` goes back to the
//! speech model's text. The earlier runs stay. Without a model the reason
//! is printed and the exit code is 0.

use diarist_core::punct::rebuild::meetings_missing_punctuation;
use diarist_core::punct::{
    rebuild_run_with, Loaded, PunctReason, PunctReport, PunctSource, RebuildWhy,
};
use diarist_core::store::Store;
use diarist_core::types::MeetingId;

use crate::{CliError, CliResult, Ctx};

/// Which meetings, and add or remove.
#[derive(Debug, clap::Args)]
#[command(group = clap::ArgGroup::new("which").required(true))]
pub struct PunctuateArgs {
    /// Meeting id or uid.
    #[arg(group = "which")]
    pub meeting: Option<String>,
    /// Every finished meeting whose transcript has no punctuation yet.
    #[arg(long, group = "which")]
    pub all_missing: bool,
    /// Remove punctuation instead (back to the speech model's text).
    #[arg(long)]
    pub remove: bool,
}

/// One line for a punctuation report, with the reason's name.
pub fn describe(r: &PunctReport) -> String {
    let model = r.model.as_deref().unwrap_or("the punctuation model");
    let msg = r.message.as_deref().unwrap_or("");
    let lang = r.language.as_deref().unwrap_or("this language");
    match r.reason {
        PunctReason::Ok => format!(
            "punctuated with {model}: {} marks, {} capitals, {} words kept as heard, {:.1} s",
            r.marks_added.unwrap_or(0),
            r.case_upgrades.unwrap_or(0),
            r.rejected_words.unwrap_or(0),
            r.wall_s.unwrap_or(0.0)
        ),
        PunctReason::Off => "punctuation is off (reason: off)".to_owned(),
        PunctReason::NoModel => "no punctuation model is installed (reason: no_model)".to_owned(),
        PunctReason::Language => {
            format!("{model} does not cover {lang} (reason: language)")
        }
        PunctReason::AsrPunctuates => {
            "the speech model already punctuates this transcript (reason: asr_punctuates)"
                .to_owned()
        }
        PunctReason::Runtime => {
            format!("the punctuation runtime did not load: {msg} (reason: runtime)")
        }
        PunctReason::Error => format!("punctuation failed: {msg} (reason: error)"),
        PunctReason::Invariant => {
            "the model's labels did not fit the words (reason: invariant)".to_owned()
        }
    }
}

pub fn punctuate(ctx: &Ctx, args: &PunctuateArgs) -> CliResult {
    let meetings: Vec<MeetingId> = match &args.meeting {
        Some(m) => {
            let id = ctx.meeting(m)?;
            ctx.store.get_meeting(id)?;
            vec![id]
        }
        None => meetings_missing_punctuation(&*ctx.store)?,
    };
    let why = if args.remove {
        RebuildWhy::Remove
    } else {
        RebuildWhy::Add
    };
    // One session for every meeting.
    let mut loaded: Loaded = match why {
        RebuildWhy::Add => PunctSource::Data(ctx.paths.clone()).load(&*ctx.store),
        RebuildWhy::Remove => Err(Box::new(PunctReport::not_applied(PunctReason::Off))),
    };
    if let (RebuildWhy::Add, Err(r)) = (why, &loaded) {
        println!("{}", describe(r));
        return Ok(());
    }
    if meetings.is_empty() {
        println!("every finished meeting is punctuated");
        return Ok(());
    }
    let mut failed = 0;
    for m in meetings {
        match rebuild_run_with(&*ctx.store, &mut loaded, m, why, &|| false) {
            Ok(r) => match r.run {
                Some(run) => println!(
                    "meeting {m}: run {run} from run {} ({})",
                    r.from_run,
                    describe(&r.punctuation)
                ),
                None => println!(
                    "meeting {m}: unchanged, run {} stays ({})",
                    r.from_run,
                    describe(&r.punctuation)
                ),
            },
            Err(e) => {
                failed += 1;
                eprintln!("meeting {m}: {e}");
            }
        }
    }
    if failed > 0 {
        return Err(CliError::Failed(format!("{failed} meetings failed")));
    }
    Ok(())
}
