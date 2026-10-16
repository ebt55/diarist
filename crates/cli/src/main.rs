//! `diarist-cli`: developer entry point to `diarist-core` without the UI.
//!
//! Exit codes: 0 ok, 1 error, 2 usage error.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{mpsc, Arc};
use std::time::Instant;

use clap::{Parser, Subcommand};

mod crash_test;
mod record;
use diarist_core::audio::capture::{self, DeviceFlow};
use diarist_core::audio::{find_on_path, ChainDecoder};
use diarist_core::engine::NemoCliEngine;
use diarist_core::export::{render, ExportDoc, ExportFormat};
use diarist_core::models::{self, DataPaths, DoctorReport, Manifest};
use diarist_core::pipeline::{import_file, ImportOptions, ImportRequest, JobQueue};
use diarist_core::store::{RunStore, SqliteStore, Store};
use diarist_core::types::{JobId, MeetingId, ModelFile, ModelRole, SegmentId, SpeakerId};
use diarist_core::views::{JobEvent, JobStage};
use diarist_core::CoreError;

#[derive(Debug, Parser)]
#[command(name = "diarist-cli", version, about = "Diarist developer CLI")]
struct Cli {
    /// Data root (default: portable.txt next to the exe, then the registry,
    /// then %LOCALAPPDATA%\app.diarist.desktop).
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    /// Database file (default: <data root>\diarist.db).
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    /// Models folder (default: <data root>\models).
    #[arg(long, global = true)]
    models_dir: Option<PathBuf>,
    /// Engine folder: runtimes, or one extracted runtime (default: <data root>\engine).
    #[arg(long, global = true)]
    engine_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print the diarist-core version.
    Version,
    /// Transcribe and diarize an audio or video file into a new meeting.
    Transcribe {
        /// Input file (m4a, mp3, wav, flac, ogg, mp4, ...).
        file: PathBuf,
        /// Meeting language (BCP-47) or `auto`. One the selected ASR model
        /// does not cover (German on the EN model) runs on Nemotron 3.5 when
        /// it is installed, else stops with what to install; --asr-model
        /// turns the switch off.
        #[arg(long, default_value = "en")]
        language: String,
        /// Meeting title (default: the file name).
        #[arg(long)]
        title: Option<String>,
        /// Transcribe without speaker diarization.
        #[arg(long)]
        no_diarize: bool,
        /// Also run the standalone diarize pass and keep its RTTM and JSON.
        #[arg(long)]
        rttm: bool,
        /// Keep the 16 kHz intermediate WAV.
        #[arg(long)]
        keep_wav: bool,
        /// Compute device for the engine.
        #[arg(long, default_value = "cuda:0")]
        device: String,
        /// Export the transcript here afterwards (format from --format or the extension).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Export format: md, txt, json, srt, vtt, rttm.
        #[arg(long)]
        format: Option<String>,
        /// Print a machine-readable result on stdout (the summary goes to stderr).
        #[arg(long)]
        json: bool,
        /// ASR model for this run only: a .gguf path or a name/id/path from
        /// `models list` (the stored selection is unchanged).
        #[arg(long, value_name = "NAME_OR_PATH")]
        asr_model: Option<String>,
        /// Diarizer for this run only (like --asr-model).
        #[arg(long, value_name = "NAME_OR_PATH")]
        diar_model: Option<String>,
        /// A flag passed verbatim to `nemo-speech transcribe` (repeatable),
        /// e.g. `--engine-arg=--verbatim`.
        #[arg(long = "engine-arg", value_name = "FLAG", allow_hyphen_values = true)]
        engine_args: Vec<String>,
    },
    /// List meetings, newest first.
    List,
    /// Show a meeting (id or uid): details, speakers and segments.
    Show {
        meeting: String,
        /// Print every segment (default: the first 20).
        #[arg(long)]
        all: bool,
    },
    /// Full-text search over the current transcripts.
    Search {
        #[arg(required = true)]
        query: Vec<String>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Export a meeting's transcript.
    Export {
        meeting: String,
        /// md, txt, json, srt, vtt, rttm (default: from --out, else md).
        #[arg(long)]
        format: Option<String>,
        /// Output file (default: stdout).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Rename a speaker; --segment records the name anchor.
    Rename {
        speaker: i64,
        name: String,
        #[arg(long)]
        segment: Option<i64>,
    },
    /// Rename a meeting.
    Title { meeting: String, title: String },
    /// Keep a meeting's audio (exempt from retention).
    Pin { meeting: String },
    /// Release a pinned meeting's audio.
    Unpin { meeting: String },
    /// Delete a meeting and its audio folder.
    Delete { meeting: String },
    /// Audio retention: list (--dry-run, the default) or delete (--apply) the
    /// playback FLAC and WAVs of unpinned meetings older than
    /// audio_retention_days. Transcripts are never touched.
    Retention {
        /// List what would be deleted (the default).
        #[arg(long, conflicts_with = "apply")]
        dry_run: bool,
        /// Delete it.
        #[arg(long)]
        apply: bool,
        /// Use this many days instead of the setting.
        #[arg(long)]
        days: Option<u32>,
    },
    /// Models: list, scan, select, pull.
    Models {
        #[command(subcommand)]
        command: ModelsCommand,
    },
    /// Runtimes: list, select, install.
    Engine {
        #[command(subcommand)]
        command: EngineCommand,
    },
    /// Data root, selections, GPU and free VRAM.
    Doctor,
    /// Audio endpoints (render endpoints are loopback sources).
    Devices,
    /// Play a 16-bit WAV to a render endpoint (dev helper for capture tests).
    Play {
        /// Render endpoint id or name (default: the default render endpoint).
        #[arg(long)]
        device: Option<String>,
        wav: PathBuf,
    },
    /// Record the mic and the system audio with live words; Ctrl-C (or
    /// --seconds) stops and runs the post-pass.
    Record {
        /// Stop after this many seconds.
        #[arg(long)]
        seconds: Option<u64>,
        #[arg(long)]
        title: Option<String>,
        /// Meeting language (BCP-47) or `auto` (default: setting default_language).
        #[arg(long)]
        language: Option<String>,
        /// Diarize the mic too (several people in the room).
        #[arg(long)]
        diarize_mic: bool,
        /// Capture endpoint id or name (default: the default communications mic).
        #[arg(long)]
        mic: Option<String>,
        /// Render endpoint id or name to loop back (default: the default
        /// output, following the endpoint meeting apps play to).
        #[arg(long)]
        loopback: Option<String>,
        /// Compute device for the engine.
        #[arg(long, default_value = "cuda:0")]
        device: String,
    },
    /// Reproduce a crash to test the local reports in <data root>\logs
    /// (docs/plans/11-hardening.md 8a).
    #[command(hide = true, group = clap::ArgGroup::new("what").required(true))]
    Crash {
        /// A Rust panic: crash-<utc>.txt.
        #[arg(long, group = "what")]
        panic: bool,
        /// An access violation: crash-<utc>.txt and .dmp.
        #[arg(long, group = "what")]
        segv: bool,
        /// The engine crashing during a job: engine-crash-<utc>.txt.
        #[arg(long, group = "what")]
        engine: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ModelsCommand {
    /// What the scanner found, by role, with VRAM fit marks.
    List,
    /// Rescan models\ and engine\.
    Scan,
    /// Select a model for a role (asr, diarizer) by id, name or file name.
    Select { role: String, query: String },
    /// Fetch models with `nemo-speech pull` (required ones by default).
    Pull {
        /// Manifest names (nemotron-3.5, nemotron-3-diarization, nemotron-en, parakeet-tdt).
        names: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
enum EngineCommand {
    /// Runtimes found under engine\.
    List,
    /// Select a runtime by version, id, name or path.
    Select { query: String },
    /// Download and install a runtime zip from the manifest.
    Install { name: Option<String> },
}

/// An error with its exit code.
enum CliError {
    Usage(String),
    Failed(String),
}

impl From<CoreError> for CliError {
    fn from(e: CoreError) -> Self {
        CliError::Failed(e.to_string())
    }
}

impl From<diarist_core::store::StoreError> for CliError {
    fn from(e: diarist_core::store::StoreError) -> Self {
        CliError::Failed(e.to_string())
    }
}

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        CliError::Failed(e.to_string())
    }
}

type CliResult<T = ()> = Result<T, CliError>;

struct Ctx {
    paths: DataPaths,
    store: Arc<SqliteStore>,
}

impl Ctx {
    fn open(cli: &Cli) -> CliResult<Self> {
        let mut paths = match &cli.data_dir {
            Some(d) => DataPaths::at(d.components().collect::<PathBuf>()),
            None => DataPaths::resolve()?,
        };
        if let Some(m) = &cli.models_dir {
            paths = paths.with_models_dir(m);
        }
        if let Some(e) = &cli.engine_dir {
            paths = paths.with_engine_dir(e);
        }
        let db = cli.db.clone().unwrap_or_else(|| paths.db());
        let store = SqliteStore::open(&db)?.with_audio_root(paths.audio());
        Ok(Self {
            paths,
            store: Arc::new(store),
        })
    }

    fn meeting(&self, r: &str) -> CliResult<MeetingId> {
        if let Ok(id) = r.parse::<i64>() {
            return Ok(MeetingId(id));
        }
        self.store
            .meeting_id_by_uid(r)?
            .ok_or_else(|| CliError::Failed(format!("no meeting with id or uid {r:?}")))
    }
}

fn main() -> ExitCode {
    if crash_test::is_stand_in_engine() {
        return crash_test::stand_in_engine();
    }
    let cli = Cli::parse();
    // Panics, native crashes and engine exits leave reports in <root>\logs.
    crash_test::install(cli.data_dir.as_deref());
    let code = match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliError::Usage(m)) => {
            eprintln!("usage error: {m}");
            ExitCode::from(2)
        }
        Err(CliError::Failed(m)) => {
            eprintln!("error: {m}");
            ExitCode::from(1)
        }
    };
    diarist_core::crash::clean_exit();
    code
}

fn run(cli: &Cli) -> CliResult {
    match &cli.command {
        Command::Version => {
            println!("diarist-cli {}", diarist_core::VERSION);
            return Ok(());
        }
        Command::Devices => return devices(),
        Command::Crash {
            panic,
            segv,
            engine,
        } => {
            return crash_test::run(cli.data_dir.as_deref(), *panic, *segv, *engine)
                .map_err(CliError::Failed)
        }
        Command::Play { device, wav } => {
            let stop = std::sync::atomic::AtomicBool::new(false);
            let r = capture::play_wav(device.as_deref(), wav, &stop)?;
            println!(
                "played {} frames on {} from qpc {}{}",
                r.frames,
                r.device_name,
                r.start_qpc_100ns,
                if r.from_device_clock {
                    " (device clock)"
                } else {
                    ""
                }
            );
            return Ok(());
        }
        _ => {}
    }
    let ctx = Ctx::open(cli)?;
    match &cli.command {
        Command::Version | Command::Devices | Command::Play { .. } | Command::Crash { .. } => {
            unreachable!()
        }
        Command::Transcribe {
            file,
            language,
            title,
            no_diarize,
            rttm,
            keep_wav,
            device,
            out,
            format,
            json,
            asr_model,
            diar_model,
            engine_args,
        } => {
            let export = export_target(format.as_deref(), out.as_deref(), true)?;
            let overrides = models::EngineOverrides {
                asr: asr_model.clone(),
                diarizer: diar_model.clone(),
                extra_args: engine_args.clone(),
            };
            transcribe(
                &ctx,
                file,
                language,
                title.clone(),
                !no_diarize,
                *rttm,
                *keep_wav,
                device,
                export,
                &overrides,
                *json,
            )
        }
        Command::List => list(&ctx),
        Command::Show { meeting, all } => show(&ctx, ctx.meeting(meeting)?, *all),
        Command::Search { query, limit } => search(&ctx, &query.join(" "), *limit),
        Command::Export {
            meeting,
            format,
            out,
        } => {
            let id = ctx.meeting(meeting)?;
            let fmt = export_target(format.as_deref(), out.as_deref(), false)?
                .map(|(f, _)| f)
                .unwrap_or(ExportFormat::Markdown);
            export(&ctx, id, fmt, out.as_deref())
        }
        Command::Rename {
            speaker,
            name,
            segment,
        } => {
            ctx.store
                .rename_speaker(SpeakerId(*speaker), name, segment.map(SegmentId))?;
            println!("speaker {speaker} renamed");
            Ok(())
        }
        Command::Title { meeting, title } => {
            ctx.store.set_meeting_title(ctx.meeting(meeting)?, title)?;
            Ok(())
        }
        Command::Pin { meeting } => {
            ctx.store.set_audio_keep(ctx.meeting(meeting)?, true)?;
            println!("pinned");
            Ok(())
        }
        Command::Unpin { meeting } => {
            ctx.store.set_audio_keep(ctx.meeting(meeting)?, false)?;
            println!("unpinned");
            Ok(())
        }
        Command::Delete { meeting } => {
            ctx.store.delete_meeting(ctx.meeting(meeting)?)?;
            println!("deleted");
            Ok(())
        }
        Command::Retention { apply, days, .. } => retention(&ctx, *apply, *days),
        Command::Models { command } => match command {
            ModelsCommand::List => models_list(&ctx),
            ModelsCommand::Scan => models_scan(&ctx),
            ModelsCommand::Select { role, query } => {
                let role = parse_role(role)?;
                let f = models::select(&*ctx.store, role, query)?;
                println!("{role}: selected {} ({})", f.display_name, f.path);
                Ok(())
            }
            ModelsCommand::Pull { names } => models_pull(&ctx, names),
        },
        Command::Engine { command } => match command {
            EngineCommand::List => engine_list(&ctx),
            EngineCommand::Select { query } => {
                let f = models::select(&*ctx.store, ModelRole::Engine, query)?;
                println!(
                    "engine: selected {} {}",
                    f.display_name,
                    f.architecture.unwrap_or_default()
                );
                Ok(())
            }
            EngineCommand::Install { name } => engine_install(&ctx, name.as_deref()),
        },
        Command::Doctor => doctor(&ctx),
        Command::Record {
            seconds,
            title,
            language,
            diarize_mic,
            mic,
            loopback,
            device,
        } => record::record(
            &ctx,
            record::RecordArgs {
                seconds: *seconds,
                title: title.clone(),
                language: language.clone(),
                diarize_mic: *diarize_mic,
                mic: mic.clone(),
                loopback: loopback.clone(),
                device: device.clone(),
            },
        ),
    }
}

fn devices() -> CliResult {
    let devs = capture::list_devices()?;
    for flow in [DeviceFlow::Capture, DeviceFlow::Render] {
        println!(
            "{}",
            if flow == DeviceFlow::Capture {
                "capture (mic):"
            } else {
                "render (loopback sources):"
            }
        );
        for d in devs.iter().filter(|d| d.flow == flow) {
            let mut marks = Vec::new();
            if d.is_default {
                marks.push("default");
            }
            if d.is_default_comms {
                marks.push("comms");
            }
            println!(
                "  {}{}
    {}",
                d.name,
                if marks.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", marks.join(", "))
                },
                d.id
            );
        }
    }
    Ok(())
}

/// Resolves `--format`/`--out` into an export target.
fn export_target(
    format: Option<&str>,
    out: Option<&Path>,
    need_out: bool,
) -> CliResult<Option<(ExportFormat, Option<PathBuf>)>> {
    let fmt = match (format, out) {
        (Some(f), _) => Some(f.parse::<ExportFormat>().map_err(|_| {
            CliError::Usage(format!(
                "unknown format {f:?} (md, txt, json, srt, vtt, rttm)"
            ))
        })?),
        (None, Some(p)) => {
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
            Some(ext.parse::<ExportFormat>().map_err(|_| {
                CliError::Usage(format!(
                    "cannot tell the format from {}; pass --format",
                    p.display()
                ))
            })?)
        }
        (None, None) => None,
    };
    if need_out && fmt.is_some() && out.is_none() {
        return Err(CliError::Usage("--format needs --out".into()));
    }
    Ok(fmt.map(|f| (f, out.map(Path::to_path_buf))))
}

fn hms(ms: i64) -> String {
    let s = ms.max(0) / 1_000;
    format!("{}:{:02}:{:02}", s / 3_600, (s / 60) % 60, s % 60)
}

fn date(ms: i64) -> String {
    diarist_core::export::utc_datetime(ms)
}

fn parse_role(s: &str) -> CliResult<ModelRole> {
    match s.to_ascii_lowercase().as_str() {
        "asr" => Ok(ModelRole::Asr),
        "diarizer" | "diarization" | "diar" => Ok(ModelRole::Diarizer),
        "engine" => Ok(ModelRole::Engine),
        other => Err(CliError::Usage(format!(
            "unknown role {other:?} (asr, diarizer, engine)"
        ))),
    }
}

#[allow(clippy::too_many_arguments)]
fn transcribe(
    ctx: &Ctx,
    file: &Path,
    language: &str,
    title: Option<String>,
    diarize: bool,
    rttm: bool,
    keep_wav: bool,
    device: &str,
    export_to: Option<(ExportFormat, Option<PathBuf>)>,
    overrides: &models::EngineOverrides,
    json: bool,
) -> CliResult {
    if !file.is_file() {
        return Err(CliError::Failed(format!(
            "{} is not a file",
            file.display()
        )));
    }
    // As the tier table spells it (`ja` -> `ja-JP`); the engine then gives
    // the runtime a tag the model's prompt dictionary has.
    let language = models::languages::canonical(language);
    let language = language.as_str();
    let (built, switched) =
        models::select_engine_for(&ctx.paths, &*ctx.store, device, language, false, overrides)?;
    let sel = built.selection;
    if let Some(reason) = switched {
        if let Some(selected) = models::selected_file(&*ctx.store, ModelRole::Asr)? {
            eprintln!("{}", models::switch_notice(reason, &selected, language));
        }
    } else if overrides.asr.is_some() && !models::languages::covers(&sel.asr_file, language) {
        eprintln!(
            "warning: {} does not list {}; transcribing with it as asked (--asr-model)",
            sel.asr_file.display_name,
            models::languages::label(language)
        );
    }
    if built.experimental {
        eprintln!(
            "warning: {} is not a streaming model; using the experimental cascade (may drop speech)",
            sel.asr_file.display_name
        );
    }
    let engine = built.engine;
    let runner = built.diarize_runner;
    let mut opts = ImportOptions::new(&ctx.paths.root, &ctx.store.get_settings()?);
    opts.language = language.to_owned();
    opts.keep_intermediate |= keep_wav;
    opts.run_diarize_pass |= rttm;
    opts.diarize_runner = Some(runner);
    let req = ImportRequest {
        job_id: JobId::new(),
        path: file.to_path_buf(),
        title,
        diarize,
    };
    eprintln!(
        "engine {} | asr {} | diarizer {}{}",
        sel.engine_version.as_deref().unwrap_or("?"),
        sel.asr_file.display_name,
        sel.diarizer_file.display_name,
        if overrides.extra_args.is_empty() {
            String::new()
        } else {
            format!(" | engine args {}", overrides.extra_args.join(" "))
        }
    );
    let decoder = Arc::new(ChainDecoder::new());
    let queue = JobQueue::new();
    let (tx, rx) = mpsc::channel();
    let store = Arc::clone(&ctx.store);
    let dec = Arc::clone(&decoder);
    let started = Instant::now();
    queue.submit(req.job_id.clone(), move |token| {
        let result = import_file(&*store, &*engine, &*dec, req, &opts, token, &|ev| {
            print_progress(&ev)
        });
        let _ = tx.send(result);
    });
    let result = rx
        .recv()
        .map_err(|_| CliError::Failed("import job ended without a result".into()))?;
    queue.wait_idle();
    eprintln!();
    let id = result?;
    let wall = started.elapsed().as_secs_f64();
    let detail = ctx.store.get_meeting(id)?;
    let words = ctx.store.get_words(id, i64::MIN, i64::MAX)?.len();
    let segments = ctx.store.get_segments(id, 0, u32::MAX)?.len();
    let duration_ms = detail.meeting.duration_ms.unwrap_or(0);
    let speakers = detail
        .speakers
        .iter()
        .filter(|s| s.merged_into.is_none())
        .count();
    // The meeting folder keeps the engine's words JSON and, with --rttm, the
    // standalone diarization (pipeline::import_file, NemoCliEngine).
    let folder = ctx.paths.audio().join(&detail.meeting.uid);
    let words_json = NemoCliEngine::output_path(&folder.join("source.wav"));
    let words_json = words_json.is_file().then_some(words_json);
    let rttm_path = folder.join("diarization.rttm");
    let rttm_path = (rttm && rttm_path.is_file()).then_some(rttm_path);
    let mut lines = vec![
        format!("meeting {} ({})", id, detail.meeting.uid),
        format!("  title     {}", detail.meeting.title),
        format!("  duration  {}", hms(duration_ms)),
        format!("  decoded   with {}", decoder.last_decoder().unwrap_or("?")),
        format!("  speakers  {speakers}"),
        format!("  segments  {segments}"),
        format!("  words     {words}"),
        format!(
            "  wall      {wall:.1} s ({:.1}x real time, RTF {:.4})",
            duration_ms as f64 / 1_000.0 / wall.max(1e-9),
            wall / (duration_ms as f64 / 1_000.0).max(1e-9)
        ),
    ];
    if let Some(p) = &words_json {
        lines.push(format!("  words json {}", p.display()));
    }
    if rttm {
        lines.push(match (&rttm_path, diarize) {
            (Some(p), _) => format!("  rttm      {}", p.display()),
            (None, false) => "  rttm      not written (--no-diarize)".to_owned(),
            (None, true) => format!(
                "  rttm      not written (expected {})",
                folder.join("diarization.rttm").display()
            ),
        });
    }
    let mut exported = None;
    if let Some((fmt, Some(out))) = export_to {
        export(ctx, id, fmt, Some(&out))?;
        lines.push(format!("  exported  {}", out.display()));
        exported = Some(out);
    }
    for l in &lines {
        if json {
            eprintln!("{l}");
        } else {
            println!("{l}");
        }
    }
    if json {
        let result = serde_json::json!({
            "meeting_id": id.0,
            "uid": detail.meeting.uid,
            "run_id": detail.current_run.as_ref().map(|r| r.id.0),
            "title": detail.meeting.title,
            "duration_ms": duration_ms,
            "speakers": speakers,
            "segments": segments,
            "words": words,
            "wall_s": (wall * 1_000.0).round() / 1_000.0,
            "decoder": decoder.last_decoder(),
            "engine_version": sel.engine_version,
            "asr_model": sel.asr.display().to_string(),
            "diar_model": sel.diarizer.display().to_string(),
            "language": language,
            "words_json": words_json.map(|p| p.display().to_string()),
            "rttm": rttm_path.map(|p| p.display().to_string()),
            "meeting_dir": folder.display().to_string(),
            "exported": exported.map(|p| p.display().to_string()),
        });
        println!("{result}");
    }
    Ok(())
}

fn print_progress(ev: &JobEvent) {
    if let JobEvent::Progress {
        stage,
        done_ms,
        total_ms,
        ..
    } = ev
    {
        let stage = match stage {
            JobStage::Queued => "queued",
            JobStage::Decoding => "decoding",
            JobStage::Transcribing => "transcribing",
            JobStage::Diarizing => "diarizing",
            JobStage::Saving => "saving",
        };
        match total_ms {
            Some(t) if *t > 0 => eprint!(
                "\r{stage:<13} {:>3}%  {} / {}   ",
                (done_ms * 100 / t).min(100),
                hms(*done_ms as i64),
                hms(*t as i64)
            ),
            _ => eprint!("\r{stage:<13}                      "),
        }
    }
}

fn list(ctx: &Ctx) -> CliResult {
    let meetings = ctx.store.list_meetings()?;
    if meetings.is_empty() {
        println!("no meetings");
    }
    for m in meetings {
        println!(
            "{:>5}  {}  {:>8}  {:>2} spk  {:<10}  {}",
            m.id.0,
            date(m.started_at),
            m.duration_ms.map(hms).unwrap_or_else(|| "-".into()),
            m.speaker_count,
            m.status.as_str(),
            m.title
        );
    }
    Ok(())
}

fn show(ctx: &Ctx, id: MeetingId, all: bool) -> CliResult {
    let d = ctx.store.get_meeting(id)?;
    let m = &d.meeting;
    println!("{} ({})", m.title, m.uid);
    println!("  status    {}", m.status.as_str());
    println!("  started   {}", date(m.started_at));
    if let Some(ms) = m.duration_ms {
        println!("  duration  {}", hms(ms));
    }
    if let Some(p) = &m.source_path {
        println!("  source    {p}");
    }
    println!("  language  {}", m.language);
    println!("  pinned    {}", m.audio_keep);
    if let Some(run) = &d.current_run {
        println!(
            "  run       {} {} ({})",
            run.kind.as_str(),
            run.status.as_str(),
            run.engine_info
                .get("runtime")
                .and_then(|v| v.as_str())
                .unwrap_or("")
        );
    }
    println!("speakers:");
    for s in &d.speakers {
        println!(
            "  {:>6}  {:<12} {:<20} {:>4} seg  {:>8}{}{}",
            s.id.0,
            s.engine_label,
            s.name,
            s.segment_count,
            hms(s.talk_ms),
            s.merged_into
                .map(|r| format!("  merged into {r}"))
                .unwrap_or_default(),
            if s.needs_review { "  (review)" } else { "" }
        );
    }
    let limit = if all { u32::MAX } else { 20 };
    let segs = ctx.store.get_segments(id, 0, limit)?;
    println!("segments:");
    for s in &segs {
        println!("  [{}] {}: {}", hms(s.start_ms), s.speaker_name, s.text);
    }
    Ok(())
}

fn search(ctx: &Ctx, query: &str, limit: u32) -> CliResult {
    let hits = ctx.store.search(query, limit)?;
    if hits.is_empty() {
        println!("no matches");
    }
    for h in hits {
        let snippet: String = h
            .snippet
            .iter()
            .map(|p| {
                if p.hit {
                    format!("[{}]", p.text)
                } else {
                    p.text.clone()
                }
            })
            .collect();
        println!(
            "{:>5} {} @ {} {}: {}",
            h.meeting_id.0,
            h.meeting_title,
            hms(h.start_ms),
            h.speaker_name,
            snippet
        );
    }
    Ok(())
}

fn export(ctx: &Ctx, id: MeetingId, fmt: ExportFormat, out: Option<&Path>) -> CliResult {
    let doc = ExportDoc::from_store(&*ctx.store, id)?;
    let text = render(&doc, fmt);
    match out {
        Some(p) => {
            if let Some(dir) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(p, text)?;
        }
        None => print!("{text}"),
    }
    Ok(())
}

fn gpu_report(ctx: &Ctx) -> Option<DoctorReport> {
    let engine = models::selected_file(&*ctx.store, ModelRole::Engine).ok()??;
    models::doctor(&ctx.paths.absolute(&engine.path)).ok()
}

fn print_files(files: &[ModelFile], ctx: &Ctx, free_mb: Option<u32>) -> CliResult {
    let settings = ctx.store.get_settings()?;
    let selected = |f: &ModelFile| {
        settings.0.get(&diarist_core::views::selection_key(f.role)) == Some(&f.id.to_string())
    };
    let mut last_role = None;
    for f in files.iter().filter(|f| f.role != ModelRole::Engine) {
        if last_role != Some(f.role) {
            println!("{}:", f.role);
            last_role = Some(f.role);
        }
        let caps = &f.capabilities;
        let (vram, kind) = match (caps.vram_mb_measured, caps.vram_mb_estimate) {
            (Some(v), _) => (Some(v), "measured"),
            (None, Some(v)) => (Some(v), "estimate"),
            _ => (None, ""),
        };
        let fit = match (vram, free_mb) {
            (Some(v), Some(free)) if f64::from(v) <= f64::from(free) * 0.9 => "fits",
            (Some(v), Some(free)) if v <= free => "tight",
            (Some(_), Some(_)) => "too big",
            _ => "",
        };
        println!(
            "  {} {:>4}  {:<44} {:<6} {:>6} MB  {:>9}  {}{}{}",
            if selected(f) { "*" } else { " " },
            f.id.0,
            f.display_name,
            f.quant.as_deref().unwrap_or("-"),
            f.size_bytes >> 20,
            vram.map(|v| format!("{v} MB {kind}")).unwrap_or_default(),
            fit,
            f.manifest_name
                .as_deref()
                .map(|n| format!("  [{n}]"))
                .unwrap_or_default(),
            if f.missing { "  MISSING" } else { "" }
        );
    }
    Ok(())
}

fn models_list(ctx: &Ctx) -> CliResult {
    let files = ctx.store.list_model_files()?;
    if files.is_empty() {
        println!("nothing scanned yet; run `diarist-cli models scan`");
        return Ok(());
    }
    let report = gpu_report(ctx);
    let free = report.as_ref().and_then(DoctorReport::free_vram_mb);
    if let Some(f) = free {
        println!("free VRAM: {f} MB");
    }
    print_files(&files, ctx, free)
}

fn models_scan(ctx: &Ctx) -> CliResult {
    let manifest = Manifest::load(&ctx.paths.root)?;
    // Runtime zips dropped into engine\ by hand: checked and extracted first.
    for r in models::download::install_dropped_engines(&ctx.paths, &manifest) {
        match r {
            Ok(dir) => println!("installed the dropped runtime zip into {}", dir.display()),
            Err(e) => println!("  problem: {e}"),
        }
    }
    let r = models::scan(&ctx.paths, &*ctx.store, &manifest)?;
    println!(
        "scanned {} and {}",
        ctx.paths.models().display(),
        ctx.paths.engine().display()
    );
    for n in &r.notes {
        println!("  {n}");
    }
    for e in &r.errors {
        println!("  problem: {e}");
    }
    print_files(&r.files, ctx, None)?;
    engine_list(ctx)
}

fn models_pull(ctx: &Ctx, names: &[String]) -> CliResult {
    let manifest = Manifest::load(&ctx.paths.root)?;
    let entries: Vec<&models::ModelEntry> = if names.is_empty() {
        manifest.required().collect()
    } else {
        names
            .iter()
            .map(|n| {
                manifest.model(n).ok_or_else(|| {
                    CliError::Usage(format!(
                        "unknown model {n:?}; known: {}",
                        manifest
                            .models
                            .iter()
                            .map(|m| m.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                })
            })
            .collect::<CliResult<_>>()?
    };
    let engine = match models::selected_file(&*ctx.store, ModelRole::Engine)? {
        Some(e) => e,
        None => {
            models::scan(&ctx.paths, &*ctx.store, &manifest)?;
            models::selected_file(&*ctx.store, ModelRole::Engine)?.ok_or_else(|| {
                CliError::Failed("no engine installed; run `diarist-cli engine install`".into())
            })?
        }
    };
    let engine_dir = ctx.paths.absolute(&engine.path);
    for e in entries {
        if e.experimental {
            eprintln!("note: {} is experimental", e.name);
        }
        eprintln!("pulling {} ({} MB)...", e.name, e.size_bytes >> 20);
        let p = match models::pull(&engine_dir, &ctx.paths.models(), e) {
            Ok(p) => p,
            Err(err) => {
                manual_download_hint(
                    &e.url,
                    &e.sha256,
                    e.size_bytes,
                    &models::pull_dir(&ctx.paths.models(), e),
                    "put the file in",
                );
                return Err(err.into());
            }
        };
        let how = if p.cached {
            "already present"
        } else {
            "downloaded"
        };
        println!("{}: {how}, SHA-256 verified", e.name);
    }
    models_scan(ctx)
}

fn engine_list(ctx: &Ctx) -> CliResult {
    let settings = ctx.store.get_settings()?;
    let sel = settings
        .0
        .get("selected_engine")
        .cloned()
        .unwrap_or_default();
    println!("engines:");
    for f in ctx
        .store
        .list_model_files()?
        .iter()
        .filter(|f| f.role == ModelRole::Engine)
    {
        let version = f.architecture.as_deref().unwrap_or("?");
        println!(
            "  {} {:>4}  {:<8} {:<10} {}{}",
            if sel == f.id.to_string() { "*" } else { " " },
            f.id.0,
            version,
            if models::TESTED_ENGINE_VERSIONS.contains(&version) {
                "tested"
            } else {
                "untested"
            },
            f.path,
            if f.missing { "  MISSING" } else { "" }
        );
    }
    Ok(())
}

/// After a failed download: what to fetch by hand and where to put it.
fn manual_download_hint(url: &str, sha256: &str, size: u64, dest: &Path, what: &str) {
    eprintln!("download failed; to fetch it by hand:");
    eprintln!("  url     {url}");
    eprintln!("  sha256  {sha256}");
    eprintln!("  size    {size} bytes");
    eprintln!("  then {what} {}", dest.display());
    eprintln!("  (a firewall such as simplewall may need to allow diarist-cli.exe, or curl.exe for model pulls)");
}

fn engine_install(ctx: &Ctx, name: Option<&str>) -> CliResult {
    let manifest = Manifest::load(&ctx.paths.root)?;
    let entry = match name {
        Some(n) => manifest
            .engines
            .iter()
            .find(|e| e.name == n || e.version == n)
            .ok_or_else(|| CliError::Usage(format!("no engine {n:?} in the manifest")))?,
        None => manifest
            .default_engine()
            .ok_or_else(|| CliError::Failed("the manifest lists no engine".into()))?,
    };
    eprintln!("downloading {} ({} MB)", entry.name, entry.size_bytes >> 20);
    let dir = models::install_engine(entry, &ctx.paths, &mut |done, total| {
        if let Some(t) = total.filter(|t| *t > 0) {
            eprint!(
                "\r{:>3}%  {} / {} MB   ",
                done * 100 / t,
                done >> 20,
                t >> 20
            );
        }
    })
    .inspect_err(|_| {
        eprintln!();
        manual_download_hint(
            &entry.url,
            &entry.sha256,
            entry.size_bytes,
            &ctx.paths.engine(),
            "put the zip in (checked and extracted by `diarist-cli models scan`)",
        );
    })?;
    eprintln!();
    println!("installed {}", dir.display());
    models_scan(ctx)
}

/// `retention`: what audio is past retention, and with `apply` deletes it.
fn retention(ctx: &Ctx, apply: bool, days: Option<u32>) -> CliResult {
    use diarist_core::retention;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let plan = match days {
        Some(d) => retention::plan(&*ctx.store, &ctx.paths.root, d, now)?,
        None => retention::plan_from_settings(&*ctx.store, &ctx.paths.root, now)?,
    };
    if plan.days == 0 {
        println!("audio_retention_days is 0: audio is kept forever");
        return Ok(());
    }
    for m in &plan.meetings {
        println!(
            "meeting {} {:?}: {} files, {:.1} MB",
            m.meeting_id,
            m.title,
            m.files.len(),
            m.files.iter().map(|f| f.bytes).sum::<u64>() as f64 / 1e6
        );
        for f in &m.files {
            println!("  {} ({} bytes)", f.path, f.bytes);
        }
    }
    println!(
        "{} meetings older than {} days: {} files, {:.1} MB{}",
        plan.meetings.len(),
        plan.days,
        plan.files,
        plan.bytes as f64 / 1e6,
        if plan.pinned_skipped > 0 {
            format!(" ({} pinned meetings kept)", plan.pinned_skipped)
        } else {
            String::new()
        }
    );
    if !apply {
        if !plan.meetings.is_empty() {
            println!(
                "dry run: nothing deleted (add --apply to delete the audio; transcripts stay)"
            );
        }
        return Ok(());
    }
    let report = retention::apply(&*ctx.store, &plan);
    println!(
        "deleted {} files ({:.1} MB) of {} meetings",
        report.files,
        report.bytes as f64 / 1e6,
        report.meetings
    );
    if !report.errors.is_empty() {
        return Err(CliError::Failed(report.errors.join("; ")));
    }
    Ok(())
}

fn doctor(ctx: &Ctx) -> CliResult {
    println!(
        "data root   {} ({:?})",
        ctx.paths.root.display(),
        ctx.paths.source
    );
    println!(
        "database    {}",
        ctx.store
            .path()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    );
    println!("models      {}", ctx.paths.models().display());
    println!("engine      {}", ctx.paths.engine().display());
    if let Some(free) = models::free_disk_bytes(&ctx.paths.root) {
        println!("free disk   {} MB", free >> 20);
    }
    match diarist_core::gpu::nvml_vram(0) {
        Some(r) => println!(
            "nvml        {} MB free of {} MB (the live start gate needs {} MB)",
            r.free_mb,
            r.total_mb.unwrap_or(0),
            diarist_core::live::MIN_FREE_VRAM_MB
        ),
        None => println!("nvml        not available (the live start gate uses doctor)"),
    }
    println!(
        "ffmpeg      {}",
        find_on_path("ffmpeg")
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "not on PATH (symphonia and Media Foundation still decode)".into())
    );
    let mut ok = true;
    for role in [ModelRole::Engine, ModelRole::Asr, ModelRole::Diarizer] {
        match models::selected_file(&*ctx.store, role)? {
            Some(f) => println!(
                "{:<11} {} {}",
                role.as_str(),
                f.display_name,
                f.architecture
                    .filter(|_| role == ModelRole::Engine)
                    .unwrap_or_default()
            ),
            None => {
                ok = false;
                println!(
                    "{:<11} none selected (run `diarist-cli models scan`)",
                    role.as_str()
                );
            }
        }
    }
    if let Some(engine) = models::selected_file(&*ctx.store, ModelRole::Engine)? {
        match models::doctor(&ctx.paths.absolute(&engine.path)) {
            Ok(r) => {
                println!("runtime     nemo-speech {}", r.version);
                if let Some(g) = r.device_info() {
                    println!(
                        "gpu         {} ({} MB free of {} MB)",
                        g.name,
                        g.vram_free_mb.unwrap_or(0),
                        g.vram_total_mb.unwrap_or(0)
                    );
                }
                println!("healthy     {}", r.healthy());
                for w in &r.runtime_warnings {
                    println!("warning     {w}");
                }
                ok &= r.healthy();
            }
            Err(e) => {
                ok = false;
                println!("runtime     doctor failed: {e}");
            }
        }
    }
    if ok {
        Ok(())
    } else {
        Err(CliError::Failed("not ready".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_transcribe() {
        let cli = Cli::try_parse_from(["diarist-cli", "transcribe", "a.wav"]).unwrap();
        assert!(
            matches!(cli.command, Command::Transcribe { ref file, ref language, .. } if file == Path::new("a.wav") && language == "en")
        );
        assert!(Cli::try_parse_from(["diarist-cli", "transcribe"]).is_err());
        let cli = Cli::try_parse_from([
            "diarist-cli",
            "--data-dir",
            "D:/x",
            "transcribe",
            "a.m4a",
            "--language",
            "auto",
            "--out",
            "o.srt",
            "--rttm",
        ])
        .unwrap();
        assert_eq!(cli.data_dir.as_deref(), Some(Path::new("D:/x")));
        assert!(matches!(
            cli.command,
            Command::Transcribe { rttm: true, .. }
        ));
        let cli = Cli::try_parse_from([
            "diarist-cli",
            "transcribe",
            "a.wav",
            "--json",
            "--asr-model",
            "nemotron-3.5",
            "--diar-model",
            "D:/m/diar.gguf",
            "--engine-arg",
            "--verbatim",
            "--engine-arg=--foo=1",
        ])
        .unwrap();
        match cli.command {
            Command::Transcribe {
                json,
                asr_model,
                diar_model,
                engine_args,
                ..
            } => {
                assert!(json);
                assert_eq!(asr_model.as_deref(), Some("nemotron-3.5"));
                assert_eq!(diar_model.as_deref(), Some("D:/m/diar.gguf"));
                assert_eq!(engine_args, ["--verbatim", "--foo=1"]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parses_the_other_commands() {
        for args in [
            vec!["diarist-cli", "list"],
            vec!["diarist-cli", "show", "3", "--all"],
            vec!["diarist-cli", "search", "budget", "review"],
            vec!["diarist-cli", "export", "3", "--format", "md"],
            vec!["diarist-cli", "rename", "7", "Priya", "--segment", "12"],
            vec!["diarist-cli", "pin", "3"],
            vec!["diarist-cli", "unpin", "3"],
            vec!["diarist-cli", "models", "list"],
            vec!["diarist-cli", "models", "scan"],
            vec!["diarist-cli", "models", "select", "asr", "nemotron-3.5"],
            vec!["diarist-cli", "models", "pull", "nemotron-en"],
            vec!["diarist-cli", "engine", "list"],
            vec!["diarist-cli", "engine", "select", "0.2.0"],
            vec!["diarist-cli", "doctor"],
            vec!["diarist-cli", "devices"],
            vec!["diarist-cli", "play", "--device", "CABLE Input", "a.wav"],
        ] {
            Cli::try_parse_from(&args).unwrap_or_else(|e| panic!("{args:?}: {e}"));
        }
    }

    #[test]
    fn parses_record() {
        let cli = Cli::try_parse_from([
            "diarist-cli",
            "record",
            "--seconds",
            "60",
            "--title",
            "Standup",
            "--language",
            "de",
            "--diarize-mic",
            "--mic",
            "CABLE Output",
            "--loopback",
            "Speakers",
        ])
        .unwrap();
        let Command::Record {
            seconds,
            title,
            language,
            diarize_mic,
            mic,
            loopback,
            device,
        } = cli.command
        else {
            panic!("expected record");
        };
        assert_eq!(seconds, Some(60));
        assert_eq!(title.as_deref(), Some("Standup"));
        assert_eq!(language.as_deref(), Some("de"));
        assert!(diarize_mic);
        assert_eq!(mic.as_deref(), Some("CABLE Output"));
        assert_eq!(loopback.as_deref(), Some("Speakers"));
        assert_eq!(device, "cuda:0");
        let bare = Cli::try_parse_from(["diarist-cli", "record"]).unwrap();
        assert!(matches!(
            bare.command,
            Command::Record { seconds: None, .. }
        ));
    }

    #[test]
    fn export_targets() {
        assert!(matches!(
            export_target(None, Some(Path::new("a.vtt")), true),
            Ok(Some((ExportFormat::Vtt, Some(_))))
        ));
        assert!(matches!(
            export_target(Some("md"), None, true),
            Err(CliError::Usage(_))
        ));
        assert!(matches!(
            export_target(Some("docx"), None, false),
            Err(CliError::Usage(_))
        ));
        assert!(matches!(export_target(None, None, false), Ok(None)));
        assert!(matches!(parse_role("diar"), Ok(ModelRole::Diarizer)));
        assert!(parse_role("gpu").is_err());
    }
}
