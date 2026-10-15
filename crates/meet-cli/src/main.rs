//! `meet-cli`: developer entry point to `meet-core` without the UI.
//!
//! Exit codes: 0 ok, 1 error, 2 usage error.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{mpsc, Arc};
use std::time::Instant;

use clap::{Parser, Subcommand};
use meet_core::audio::{find_on_path, ChainDecoder};
use meet_core::engine::{DiarizeRunner, Engine, NemoCascadeEngine, NemoCliEngine};
use meet_core::export::{render, ExportDoc, ExportFormat};
use meet_core::models::{self, DataPaths, DoctorReport, Manifest};
use meet_core::pipeline::{import_file, ImportOptions, ImportRequest, JobQueue};
use meet_core::store::{RunStore, SqliteStore, Store};
use meet_core::types::{JobId, MeetingId, ModelFile, ModelRole, SegmentId, SpeakerId};
use meet_core::views::{JobEvent, JobStage};
use meet_core::CoreError;

#[derive(Debug, Parser)]
#[command(name = "meet-cli", version, about = "meet-assist developer CLI")]
struct Cli {
    /// Data root (default: portable.txt next to the exe, then the registry,
    /// then %LOCALAPPDATA%\app.meetassist.desktop).
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    /// Database file (default: <data root>\meet-assist.db).
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
    /// Print the meet-core version.
    Version,
    /// Transcribe and diarize an audio or video file into a new meeting.
    Transcribe {
        /// Input file (m4a, mp3, wav, flac, ogg, mp4, ...).
        file: PathBuf,
        /// Meeting language (BCP-47) or `auto`.
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

impl From<meet_core::store::StoreError> for CliError {
    fn from(e: meet_core::store::StoreError) -> Self {
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
            Some(d) => DataPaths::at(d),
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
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliError::Usage(m)) => {
            eprintln!("usage error: {m}");
            ExitCode::from(2)
        }
        Err(CliError::Failed(m)) => {
            eprintln!("error: {m}");
            ExitCode::from(1)
        }
    }
}

fn run(cli: &Cli) -> CliResult {
    if let Command::Version = cli.command {
        println!("meet-cli {}", meet_core::VERSION);
        return Ok(());
    }
    let ctx = Ctx::open(cli)?;
    match &cli.command {
        Command::Version => unreachable!(),
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
        } => {
            let export = export_target(format.as_deref(), out.as_deref(), true)?;
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
    }
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
    meet_core::export::utc_datetime(ms)
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

/// The selection, scanning first when nothing usable is selected yet.
fn selection(ctx: &Ctx) -> CliResult<models::ResolvedSelection> {
    match models::resolve_selection(&ctx.paths, &*ctx.store) {
        Ok(s) => Ok(s),
        Err(_) => {
            let manifest = Manifest::load(&ctx.paths.root)?;
            let report = models::scan(&ctx.paths, &*ctx.store, &manifest)?;
            for n in &report.notes {
                eprintln!("{n}");
            }
            Ok(models::resolve_selection(&ctx.paths, &*ctx.store)?)
        }
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
) -> CliResult {
    if !file.is_file() {
        return Err(CliError::Failed(format!(
            "{} is not a file",
            file.display()
        )));
    }
    let sel = selection(ctx)?;
    let models_dir = ctx.paths.models();
    let engine: Arc<dyn Engine> = if sel.asr_file.capabilities.streaming {
        let mut e = NemoCliEngine::new(&sel.engine_dir, &models_dir, &sel.asr, &sel.diarizer);
        e.device = device.to_owned();
        e.version = sel.engine_version.clone();
        Arc::new(e)
    } else {
        eprintln!(
            "warning: {} is not a streaming model; using the experimental cascade (may drop speech)",
            sel.asr_file.display_name
        );
        let mut e = NemoCascadeEngine::new(&sel.engine_dir, &models_dir, &sel.asr, &sel.diarizer);
        e.device = device.to_owned();
        e.version = sel.engine_version.clone();
        Arc::new(e)
    };
    let mut runner = DiarizeRunner::new(&sel.engine_dir, &models_dir, &sel.diarizer);
    runner.device = device.to_owned();
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
        "engine {} | asr {} | diarizer {}",
        sel.engine_version.as_deref().unwrap_or("?"),
        sel.asr_file.display_name,
        sel.diarizer_file.display_name
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
    println!("meeting {} ({})", id, detail.meeting.uid);
    println!("  title     {}", detail.meeting.title);
    println!("  duration  {}", hms(duration_ms));
    println!("  decoded   with {}", decoder.last_decoder().unwrap_or("?"));
    println!("  speakers  {speakers}");
    println!("  segments  {segments}");
    println!("  words     {words}");
    println!(
        "  wall      {wall:.1} s ({:.1}x real time, RTF {:.4})",
        duration_ms as f64 / 1_000.0 / wall.max(1e-9),
        wall / (duration_ms as f64 / 1_000.0).max(1e-9)
    );
    if let Some((fmt, Some(out))) = export_to {
        export(ctx, id, fmt, Some(&out))?;
        println!("  exported  {}", out.display());
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
        settings.0.get(&meet_core::views::selection_key(f.role)) == Some(&f.id.to_string())
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
        println!("nothing scanned yet; run `meet-cli models scan`");
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
                CliError::Failed("no engine installed; run `meet-cli engine install`".into())
            })?
        }
    };
    let engine_dir = ctx.paths.absolute(&engine.path);
    for e in entries {
        if e.experimental {
            eprintln!("note: {} is experimental", e.name);
        }
        eprintln!("pulling {} ({} MB)...", e.name, e.size_bytes >> 20);
        let p = models::pull(&engine_dir, &ctx.paths.models(), e)?;
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
    })?;
    eprintln!();
    println!("installed {}", dir.display());
    models_scan(ctx)
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
                    "{:<11} none selected (run `meet-cli models scan`)",
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
        let cli = Cli::try_parse_from(["meet-cli", "transcribe", "a.wav"]).unwrap();
        assert!(
            matches!(cli.command, Command::Transcribe { ref file, ref language, .. } if file == Path::new("a.wav") && language == "en")
        );
        assert!(Cli::try_parse_from(["meet-cli", "transcribe"]).is_err());
        let cli = Cli::try_parse_from([
            "meet-cli",
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
    }

    #[test]
    fn parses_the_other_commands() {
        for args in [
            vec!["meet-cli", "list"],
            vec!["meet-cli", "show", "3", "--all"],
            vec!["meet-cli", "search", "budget", "review"],
            vec!["meet-cli", "export", "3", "--format", "md"],
            vec!["meet-cli", "rename", "7", "Priya", "--segment", "12"],
            vec!["meet-cli", "pin", "3"],
            vec!["meet-cli", "unpin", "3"],
            vec!["meet-cli", "models", "list"],
            vec!["meet-cli", "models", "scan"],
            vec!["meet-cli", "models", "select", "asr", "nemotron-3.5"],
            vec!["meet-cli", "models", "pull", "nemotron-en"],
            vec!["meet-cli", "engine", "list"],
            vec!["meet-cli", "engine", "select", "0.2.0"],
            vec!["meet-cli", "doctor"],
        ] {
            Cli::try_parse_from(&args).unwrap_or_else(|e| panic!("{args:?}: {e}"));
        }
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
