//! The ONNX Runtime punctuator (docs/plans/16 sections 3.6 and 3.7): one
//! generic runner for any token classifier its description block
//! ([`PunctSpec`]) describes, on the CPU execution provider only.
//!
//! Loading, once per process (ort's environment is process-global, so the
//! first library loaded is the one every session uses; a model folder with
//! another library takes effect after a restart):
//!
//! 1. The library is the model folder's `diarist-onnxruntime.dll` (renamed,
//!    so the Windows loader never confuses it with another copy, such as
//!    System32's 1.17), else one under `engine\onnxruntime-*\`.
//! 2. Its Visual C++ runtime dependencies are probed in the system folder;
//!    a missing one is named in the error when the load then fails.
//! 3. `ort::init_from(<absolute path>)` loads it (`api-31`: an older library
//!    makes the load fail with ort's own message).
//! 4. Module identity: the module that holds the API's functions must be
//!    the file we loaded (`GetModuleHandleExW` from a function address,
//!    then `GetModuleFileNameW`), and its version string must start with
//!    the block's `runtime_version`.
//!
//! Any failure gives [`Unavailable::runtime`], so the run records
//! `reason: "runtime"` and Diagnostics shows the "Punctuation runtime" row.
//! The session is created per punctuator and dropped with it.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ort::environment::Environment;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::{Session, SessionOutputs};
use ort::value::Tensor;

use super::spec::{
    parse_case_label, rel_path, HeadKind, HeadPosition, HeadValues, InputCasing, MarkLabel,
    PunctHead, PunctSpec,
};
use super::tokenize::HfTokenizer;
use super::windows::{run_windows, Encoded, TokenCounter, WindowOpts};
use super::{
    Case, LabelStats, LangHint, PunctModelInfo, PunctOptions, PunctRuntime, Punctuator, Script,
    Unavailable, WordLabel,
};
use crate::error::{CoreError, Result};
use crate::models::{self, DataPaths, Manifest};
use crate::store::Store;
use crate::types::ModelRole;

/// Microsoft's page for the Visual C++ Redistributable (Diagnostics links it).
pub const VC_REDIST_URL: &str =
    "https://learn.microsoft.com/cpp/windows/latest-supported-vc-redist";

/// What `onnxruntime.dll` 1.31.0 imports beyond Windows itself (`dumpbin
/// /dependents`, plan 16 section 3.7). `nemo-speech.exe` imports the same
/// family but ships its own copies in its `bin\`, and `diarist.exe` links
/// the C runtime statically, so neither proves they are installed.
pub const RUNTIME_DEPENDENCIES: &[&str] = &[
    "VCRUNTIME140.dll",
    "VCRUNTIME140_1.dll",
    "MSVCP140.dll",
    "MSVCP140_1.dll",
];

/// The ONNX Runtime library of this process, once loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedRuntime {
    pub path: PathBuf,
    /// `OrtGetApiBase()->GetVersionString()`, e.g. `1.31.0`.
    pub version: String,
}

enum Attempt {
    Ready {
        runtime: LoadedRuntime,
        env: Environment,
    },
    /// Loaded (it stays in the process) but a check failed.
    Broken { path: PathBuf, message: String },
}

static STATE: Mutex<Option<Attempt>> = Mutex::new(None);

/// The library loaded in this process and its version, if any.
pub fn loaded_runtime() -> Option<LoadedRuntime> {
    match &*STATE.lock().unwrap_or_else(|p| p.into_inner()) {
        Some(Attempt::Ready { runtime, .. }) => Some(runtime.clone()),
        _ => None,
    }
}

/// Loads (once) and checks the library at `dll`; see the module docs.
pub fn ensure_runtime(
    dll: &Path,
    want_version: Option<&str>,
) -> std::result::Result<(LoadedRuntime, Environment), Unavailable> {
    let mut state = STATE.lock().unwrap_or_else(|p| p.into_inner());
    match &*state {
        Some(Attempt::Ready { runtime, env }) => {
            if !same_file(&runtime.path, dll) {
                return Err(Unavailable::runtime(format!(
                    "ONNX Runtime is already loaded from {}; restart Diarist to use {}",
                    runtime.path.display(),
                    dll.display()
                )));
            }
            check_version(&runtime.version, want_version, dll)?;
            return Ok((runtime.clone(), env.clone()));
        }
        Some(Attempt::Broken { path, message }) => {
            return Err(Unavailable::runtime(if same_file(path, dll) {
                message.clone()
            } else {
                format!(
                    "{message}; another ONNX Runtime ({}) can be used after a restart",
                    dll.display()
                )
            }));
        }
        None => {}
    }
    if !dll.is_file() {
        return Err(Unavailable::runtime(format!(
            "the ONNX Runtime library {} is missing",
            dll.display()
        )));
    }
    let missing = missing_dependency();
    let builder = match ort::init_from(dll) {
        Ok(b) => b,
        // Nothing stays loaded after a failed init: a later call may retry.
        Err(e) => {
            let mut msg = format!("cannot load {}: {e}", dll.display());
            if let Some(d) = missing {
                msg.push_str(&format!(
                    "; {d} is missing: install the Microsoft Visual C++ Redistributable (x64), {VC_REDIST_URL}"
                ));
            }
            return Err(Unavailable::runtime(msg));
        }
    };
    let checked = (|| {
        let module = api_module_path()?;
        identity_check(dll, &module)?;
        let version = api_version()?;
        check_version(&version, want_version, dll)?;
        let env = builder
            .with_name("diarist-punctuation")
            .with_telemetry(false)
            .build()
            .map_err(|e| Unavailable::runtime(format!("ONNX Runtime environment: {e}")))?;
        Ok::<_, Unavailable>((
            LoadedRuntime {
                path: dll.to_path_buf(),
                version,
            },
            env,
        ))
    })();
    match checked {
        Ok((runtime, env)) => {
            log::info!(
                "punctuation: ONNX Runtime {} loaded from {}",
                runtime.version,
                runtime.path.display()
            );
            *state = Some(Attempt::Ready {
                runtime: runtime.clone(),
                env: env.clone(),
            });
            Ok((runtime, env))
        }
        Err(u) => {
            log::error!("punctuation: {}", u.message);
            *state = Some(Attempt::Broken {
                path: dll.to_path_buf(),
                message: u.message.clone(),
            });
            Err(u)
        }
    }
}

/// The module check: `module` (the file the API's functions live in) must
/// be `loaded` (the file we asked for). Both paths are in the message.
pub fn identity_check(loaded: &Path, module: &Path) -> std::result::Result<(), Unavailable> {
    if same_file(loaded, module) {
        Ok(())
    } else {
        Err(Unavailable::runtime(format!(
            "the ONNX Runtime functions come from {} instead of {}; another copy of ONNX Runtime is in the process",
            module.display(),
            loaded.display()
        )))
    }
}

/// The version check: `version` must start with `want` (when given).
pub fn check_version(
    version: &str,
    want: Option<&str>,
    dll: &Path,
) -> std::result::Result<(), Unavailable> {
    match want.map(str::trim).filter(|w| !w.is_empty()) {
        Some(w) if !version.starts_with(w) => Err(Unavailable::runtime(format!(
            "{} is ONNX Runtime {version}, the model needs {w}",
            dll.display()
        ))),
        _ => Ok(()),
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x
            .to_string_lossy()
            .eq_ignore_ascii_case(&y.to_string_lossy()),
        _ => a == b,
    }
}

/// The first of [`RUNTIME_DEPENDENCIES`] the system folder lacks.
pub fn missing_dependency() -> Option<&'static str> {
    RUNTIME_DEPENDENCIES
        .iter()
        .copied()
        .find(|d| !system_dll_present(d))
}

#[cfg(windows)]
pub(crate) fn system_dll_present(name: &str) -> bool {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::FreeLibrary;
    use windows::Win32::System::LibraryLoader::{LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};
    // SAFETY: loads a system DLL by name from System32 only and frees it.
    unsafe {
        match LoadLibraryExW(&HSTRING::from(name), None, LOAD_LIBRARY_SEARCH_SYSTEM32) {
            Ok(h) => {
                let _ = FreeLibrary(h);
                true
            }
            Err(_) => false,
        }
    }
}

#[cfg(not(windows))]
pub(crate) fn system_dll_present(_name: &str) -> bool {
    true
}

/// The file holding the ONNX Runtime API's functions (call only after a
/// successful `init_from`).
#[cfg(windows)]
fn api_module() -> std::result::Result<windows::Win32::Foundation::HMODULE, Unavailable> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::System::LibraryLoader::{
        GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
        GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    };
    let addr = ort::api().CreateStatus as *const u16;
    let mut module = HMODULE::default();
    // SAFETY: FROM_ADDRESS treats the name as an address inside a module;
    // UNCHANGED_REFCOUNT takes no reference (the library stays loaded for
    // the process).
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(addr),
            &mut module,
        )
    }
    .map_err(|e| Unavailable::runtime(format!("cannot find the ONNX Runtime module: {e}")))?;
    Ok(module)
}

#[cfg(windows)]
fn api_module_path() -> std::result::Result<PathBuf, Unavailable> {
    use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
    let module = api_module()?;
    let mut buf = vec![0u16; 32_768];
    // SAFETY: the buffer is valid for its length.
    let n = unsafe { GetModuleFileNameW(Some(module), &mut buf) } as usize;
    if n == 0 || n >= buf.len() {
        return Err(Unavailable::runtime(
            "cannot read the ONNX Runtime module's file name",
        ));
    }
    Ok(PathBuf::from(String::from_utf16_lossy(&buf[..n])))
}

#[cfg(windows)]
fn api_version() -> std::result::Result<String, Unavailable> {
    use windows::core::s;
    use windows::Win32::System::LibraryLoader::GetProcAddress;
    let module = api_module()?;
    // SAFETY: `OrtGetApiBase` takes nothing and returns a static struct
    // whose `GetVersionString` returns a static C string.
    unsafe {
        let Some(f) = GetProcAddress(module, s!("OrtGetApiBase")) else {
            return Err(Unavailable::runtime(
                "the ONNX Runtime module has no OrtGetApiBase",
            ));
        };
        let get_base: unsafe extern "system" fn() -> *const ort::sys::OrtApiBase =
            std::mem::transmute(f);
        let base = get_base();
        if base.is_null() {
            return Err(Unavailable::runtime("OrtGetApiBase returned nothing"));
        }
        let v = ((*base).GetVersionString)();
        if v.is_null() {
            return Err(Unavailable::runtime("ONNX Runtime has no version string"));
        }
        Ok(std::ffi::CStr::from_ptr(v).to_string_lossy().into_owned())
    }
}

#[cfg(not(windows))]
fn api_module_path() -> std::result::Result<PathBuf, Unavailable> {
    Err(Unavailable::runtime(
        "the punctuation runtime is supported on Windows only",
    ))
}

#[cfg(not(windows))]
fn api_version() -> std::result::Result<String, Unavailable> {
    Err(Unavailable::runtime(
        "the punctuation runtime is supported on Windows only",
    ))
}

/// The library for a model folder: the block's `runtime` file in the folder
/// (default `diarist-onnxruntime.dll`), else a shared copy under
/// `engine\onnxruntime-<version>\` (`diarist-onnxruntime.dll`,
/// `onnxruntime.dll` or `lib\onnxruntime.dll`; a folder whose name has the
/// wanted version first). `Err` lists where it looked.
pub fn find_runtime(
    root: &Path,
    spec: &PunctSpec,
    paths: &DataPaths,
) -> std::result::Result<PathBuf, Unavailable> {
    let own = rel_path(root, spec.runtime_file());
    if own.is_file() {
        return Ok(own);
    }
    let engine = paths.engine();
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&engine)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_dir()
                        && p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.to_ascii_lowercase().starts_with("onnxruntime"))
                })
                .collect()
        })
        .unwrap_or_default();
    let want = spec.runtime_version.clone().unwrap_or_default();
    dirs.sort_by_key(|d| {
        let name = d.file_name().map(|n| n.to_string_lossy().into_owned());
        (
            !(want.is_empty() || name.as_deref().is_some_and(|n| n.contains(want.as_str()))),
            name,
        )
    });
    for d in &dirs {
        for f in [
            super::spec::RUNTIME_DLL,
            "onnxruntime.dll",
            "lib/onnxruntime.dll",
        ] {
            let p = rel_path(d, f);
            if p.is_file() {
                return Ok(p);
            }
        }
    }
    Err(Unavailable::runtime(format!(
        "the ONNX Runtime library is missing: not at {} nor under {}",
        own.display(),
        engine.join("onnxruntime-<version>").display()
    )))
}

/// One token's outputs.
#[derive(Debug, Clone, Default)]
struct TokOut {
    post: u32,
    pre: u32,
    case: u32,
    /// Per-character casing flags (a `[batch, tokens, chars]` case head).
    chars: Option<Vec<bool>>,
}

/// A head's classes, parsed once.
struct Head {
    output: String,
    position: HeadPosition,
    values: HeadValues,
    marks: Vec<MarkLabel>,
    cases: Vec<Case>,
}

impl Head {
    fn new(h: &PunctHead) -> Self {
        Self {
            output: h.output.clone(),
            position: h.position,
            values: h.values,
            marks: h
                .labels
                .iter()
                .map(|l| MarkLabel::parse(l).unwrap_or(MarkLabel::None))
                .collect(),
            cases: h
                .labels
                .iter()
                .map(|l| parse_case_label(l).unwrap_or_default())
                .collect(),
        }
    }

    fn mark(&self, class: u32) -> MarkLabel {
        self.marks
            .get(class as usize)
            .copied()
            .unwrap_or(MarkLabel::None)
    }
}

/// A punctuation model in ONNX Runtime.
pub struct OnnxPunctuator {
    session: Session,
    tokenizer: HfTokenizer,
    spec: PunctSpec,
    info: PunctModelInfo,
    threads: u32,
    post: Option<Head>,
    pre: Option<Head>,
    case: Option<Head>,
    stats: LabelStats,
}

impl std::fmt::Debug for OnnxPunctuator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnnxPunctuator")
            .field("info", &self.info)
            .field("threads", &self.threads)
            .finish_non_exhaustive()
    }
}

fn model_err(what: &str, e: impl std::fmt::Display) -> Unavailable {
    Unavailable::error(format!("punctuation model: {what}: {e}"))
}

impl OnnxPunctuator {
    /// Loads the runtime (once per process), the tokenizer and a session
    /// for the `.onnx` file at `model`. `info.runtime` is filled in.
    pub fn open(
        model: &Path,
        spec: PunctSpec,
        mut info: PunctModelInfo,
        paths: &DataPaths,
        threads: u32,
    ) -> std::result::Result<Self, Unavailable> {
        spec.validate().map_err(|e| model_err("description", e))?;
        let root = spec
            .model_root(model, &paths.models())
            .map_err(|e| model_err("folder", e))?;
        let dll = find_runtime(&root, &spec, paths)?;
        let (runtime, env) = ensure_runtime(&dll, spec.runtime_version.as_deref())?;
        info.runtime = PunctRuntime {
            name: "onnxruntime".into(),
            version: runtime.version.clone(),
            path: paths.relative(&runtime.path),
        };
        let tokenizer = HfTokenizer::load(&rel_path(&root, &spec.tokenizer))
            .map_err(|e| model_err("tokenizer", e))?;
        let threads = threads.max(1);
        let session = Session::builder(&env)
            .map_err(|e| model_err("session", e))?
            .with_optimization_level(GraphOptimizationLevel::All)
            .and_then(|b| b.with_intra_threads(threads as usize))
            .and_then(|b| b.with_inter_threads(1))
            .map_err(|e| model_err("session options", e))?
            .commit_from_file(model)
            .map_err(|e| model_err(&model.display().to_string(), e))?;
        let has_input = |n: &str| session.inputs().iter().any(|i| i.name() == n);
        let has_output = |n: &str| session.outputs().iter().any(|o| o.name() == n);
        for name in std::iter::once(&spec.inputs.ids).chain(spec.inputs.mask.as_ref()) {
            if !has_input(name) {
                return Err(model_err(
                    "inputs",
                    format!(
                        "the model has no input {name:?} (it has {:?})",
                        session
                            .inputs()
                            .iter()
                            .map(|i| i.name())
                            .collect::<Vec<_>>()
                    ),
                ));
            }
        }
        for h in &spec.heads {
            if h.kind != HeadKind::Boundary && !has_output(&h.output) {
                return Err(model_err(
                    "heads",
                    format!(
                        "the model has no output {:?} (it has {:?})",
                        h.output,
                        session
                            .outputs()
                            .iter()
                            .map(|o| o.name())
                            .collect::<Vec<_>>()
                    ),
                ));
            }
        }
        let head = |k: HeadKind| spec.head(k).map(Head::new);
        Ok(Self {
            post: head(HeadKind::Post),
            pre: head(HeadKind::Pre),
            case: head(HeadKind::Case),
            session,
            tokenizer,
            spec,
            info,
            threads,
            stats: LabelStats::default(),
        })
    }

    fn window_opts(&self) -> WindowOpts {
        WindowOpts {
            max_tokens: self.spec.max_tokens as usize,
            specials: self.spec.specials() as usize,
            context: self.spec.context() as usize,
            batch: self.spec.batch_size() as usize,
        }
    }

    /// One model call over a batch of windows: one output per token.
    fn infer(&mut self, batch: &[&[i64]]) -> Result<Vec<Vec<TokOut>>> {
        let spec = &self.spec;
        let off = usize::from(spec.bos_id.is_some());
        let t = batch.iter().map(|w| w.len()).max().unwrap_or(0) + spec.specials() as usize;
        let b = batch.len();
        let mut ids = vec![spec.pad_id; b * t];
        let mut mask = vec![0i64; b * t];
        for (r, w) in batch.iter().enumerate() {
            let row = &mut ids[r * t..(r + 1) * t];
            let mut k = 0;
            if let Some(bos) = spec.bos_id {
                row[0] = bos;
                k = 1;
            }
            row[k..k + w.len()].copy_from_slice(w);
            k += w.len();
            if let Some(eos) = spec.eos_id {
                row[k] = eos;
                k += 1;
            }
            mask[r * t..r * t + k].fill(1);
        }
        let shape = vec![b as i64, t as i64];
        let ort_err = |e: ort::Error| CoreError::Models(format!("punctuation model: {e}"));
        let mut inputs: Vec<(String, ort::value::DynValue)> = vec![(
            spec.inputs.ids.clone(),
            Tensor::from_array((shape.clone(), ids))
                .map_err(ort_err)?
                .into_dyn(),
        )];
        if let Some(m) = &spec.inputs.mask {
            inputs.push((
                m.clone(),
                Tensor::from_array((shape, mask))
                    .map_err(ort_err)?
                    .into_dyn(),
            ));
        }
        let heads = [&self.post, &self.pre, &self.case];
        let outputs = self.session.run(inputs).map_err(ort_err)?;
        let mut out: Vec<Vec<TokOut>> = batch
            .iter()
            .map(|w| vec![TokOut::default(); w.len()])
            .collect();
        for (slot, head) in heads.into_iter().enumerate() {
            let Some(head) = head else { continue };
            let values = read_head(&outputs, head, b, t)?;
            if slot != 2 && matches!(values, HeadOut::Chars { .. }) {
                return Err(CoreError::Models(format!(
                    "punctuation model output {:?}: per-character values are for a case head only",
                    head.output
                )));
            }
            for (r, w) in batch.iter().enumerate() {
                for p in 0..w.len() {
                    let at = r * t + off + p;
                    let o = &mut out[r][p];
                    match (&values, slot) {
                        (HeadOut::Classes(v), 0) => o.post = v[at],
                        (HeadOut::Classes(v), 1) => o.pre = v[at],
                        (HeadOut::Classes(v), _) => o.case = v[at],
                        (HeadOut::Chars { c, flags }, _) => {
                            o.chars = Some(flags[at * c..(at + 1) * c].to_vec())
                        }
                    }
                }
            }
        }
        Ok(out)
    }

    /// The word labels from the tokens' outputs.
    fn word_labels(
        &mut self,
        model_words: &[String],
        enc: &Encoded,
        toks: &[TokOut],
    ) -> Vec<WordLabel> {
        let mut labels = vec![WordLabel::KEEP; model_words.len()];
        for (w, r) in enc.word_ranges(model_words.len()).into_iter().enumerate() {
            if r.start == r.end {
                continue;
            }
            let (first, last) = (r.start, r.end - 1);
            let at_end = enc.ends_word[last];
            let pick = |pos: HeadPosition, of_pre: bool| match pos {
                HeadPosition::FirstToken => first,
                HeadPosition::LastToken => last,
                HeadPosition::PerSubword if of_pre => first,
                HeadPosition::PerSubword => last,
            };
            let label = &mut labels[w];
            if let Some(h) = &self.post {
                if h.position == HeadPosition::PerSubword {
                    let inner = (first..last)
                        .filter(|&k| matches!(h.mark(toks[k].post), MarkLabel::Mark(_)))
                        .count();
                    self.stats.inner_marks_dropped += inner as u64;
                }
                let k = pick(h.position, false);
                if at_end || h.position == HeadPosition::FirstToken {
                    if let MarkLabel::Mark(m) = h.mark(toks[k].post) {
                        if !m.is_pre() {
                            label.post = Some(m);
                        }
                    }
                } else if matches!(h.mark(toks[k].post), MarkLabel::Mark(_)) {
                    // The word's last token runs into the next word.
                    self.stats.inner_marks_dropped += 1;
                }
            }
            if let Some(h) = &self.pre {
                if let MarkLabel::Mark(m) = h.mark(toks[pick(h.position, true)].pre) {
                    if m.is_pre() {
                        label.pre = Some(m);
                    }
                }
            }
            if let Some(h) = &self.case {
                label.case = if toks[first].chars.is_some() {
                    char_case(&model_words[w], &enc.pieces[r.clone()], &toks[r.clone()])
                } else {
                    h.cases
                        .get(toks[pick(h.position, true)].case as usize)
                        .cloned()
                        .unwrap_or_default()
                };
            }
        }
        labels
    }
}

/// Per-character casing of one word from its tokens' flags: one flag per
/// character when the pieces spell the word as the model saw it, else the
/// first letter's flag only (SentencePiece's normalisation can change the
/// characters, plan 16 section 3.3 rule 6).
fn char_case(model_word: &str, pieces: &[String], toks: &[TokOut]) -> Case {
    let mut spelled = String::new();
    let mut flags = Vec::new();
    for (piece, t) in pieces.iter().zip(toks) {
        let f = t.chars.as_deref().unwrap_or(&[]);
        for (i, ch) in piece.chars().enumerate() {
            spelled.push(ch);
            flags.push(f.get(i).copied().unwrap_or(false));
        }
    }
    if !flags.iter().any(|&f| f) {
        return Case::Keep;
    }
    if spelled == model_word.to_lowercase() || spelled == model_word {
        if flags.iter().all(|&f| f) && flags.len() > 1 {
            Case::Upper
        } else if flags[0] && flags[1..].iter().all(|&f| !f) {
            Case::First
        } else {
            Case::Chars(flags)
        }
    } else if flags[0] {
        Case::First
    } else {
        Case::Keep
    }
}

/// One head's output for a batch.
enum HeadOut {
    /// A class per token, `[batch * tokens]`.
    Classes(Vec<u32>),
    /// Flags per token and character, `[batch * tokens * c]`.
    Chars { c: usize, flags: Vec<bool> },
}

fn read_head(outputs: &SessionOutputs<'_>, head: &Head, b: usize, t: usize) -> Result<HeadOut> {
    let bad = |what: String| {
        CoreError::Models(format!(
            "punctuation model output {:?}: {what}",
            head.output
        ))
    };
    let v = outputs
        .get(&head.output)
        .ok_or_else(|| bad("missing".into()))?;
    let dims = |shape: &[i64]| -> Vec<usize> { shape.iter().map(|&d| d.max(0) as usize).collect() };
    match head.values {
        HeadValues::Logits => {
            let (shape, data) = v
                .try_extract_tensor::<f32>()
                .map_err(|e| bad(format!("expected float logits: {e}")))?;
            let d = dims(shape);
            if d.len() != 3 || d[0] != b || d[1] != t || d[2] == 0 {
                return Err(bad(format!("shape {d:?}, expected [{b}, {t}, classes]")));
            }
            let classes = d[2];
            Ok(HeadOut::Classes(
                data.chunks(classes)
                    .map(|row| {
                        let mut best = 0;
                        for (i, x) in row.iter().enumerate() {
                            if *x > row[best] {
                                best = i;
                            }
                        }
                        best as u32
                    })
                    .collect(),
            ))
        }
        HeadValues::Decisions => {
            let (d, vals) = decisions(v).map_err(bad)?;
            match d.len() {
                2 if d[0] == b && d[1] == t => Ok(HeadOut::Classes(vals)),
                3 if d[0] == b && d[1] == t => Ok(HeadOut::Chars {
                    c: d[2],
                    flags: vals.into_iter().map(|x| x != 0).collect(),
                }),
                _ => Err(bad(format!(
                    "shape {d:?}, expected [{b}, {t}] or [{b}, {t}, chars]"
                ))),
            }
        }
    }
}

/// Integer or boolean decisions as classes, with the shape.
fn decisions(v: &ort::value::DynValue) -> std::result::Result<(Vec<usize>, Vec<u32>), String> {
    let dims = |shape: &[i64]| -> Vec<usize> { shape.iter().map(|&d| d.max(0) as usize).collect() };
    if let Ok((s, d)) = v.try_extract_tensor::<i64>() {
        return Ok((
            dims(s),
            d.iter()
                .map(|&x| x.clamp(0, u32::MAX.into()) as u32)
                .collect(),
        ));
    }
    if let Ok((s, d)) = v.try_extract_tensor::<i32>() {
        return Ok((dims(s), d.iter().map(|&x| x.max(0) as u32).collect()));
    }
    if let Ok((s, d)) = v.try_extract_tensor::<bool>() {
        return Ok((dims(s), d.iter().map(|&x| u32::from(x)).collect()));
    }
    if let Ok((s, d)) = v.try_extract_tensor::<u8>() {
        return Ok((dims(s), d.iter().map(|&x| u32::from(x)).collect()));
    }
    Err("expected int64, int32, bool or uint8 decisions".into())
}

impl Punctuator for OnnxPunctuator {
    fn descriptor(&self) -> &PunctModelInfo {
        &self.info
    }

    fn label_stream(
        &mut self,
        words: &[&str],
        hint: &LangHint,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Vec<WordLabel>> {
        if words.is_empty() {
            return Ok(Vec::new());
        }
        let model_words: Vec<String> = words
            .iter()
            .map(|w| match self.spec.input_casing {
                InputCasing::Keep => (*w).to_owned(),
                InputCasing::Lower => w.to_lowercase(),
            })
            .collect();
        let refs: Vec<&str> = model_words.iter().map(String::as_str).collect();
        let sep = if matches!(
            hint.script,
            Script::Han | Script::Hiragana | Script::Katakana
        ) {
            ""
        } else {
            " "
        };
        let enc = self.tokenizer.encode(&refs, sep)?;
        let opts = self.window_opts();
        let toks = run_windows(&[&enc], &opts, cancel, &mut |batch| self.infer(batch))?
            .pop()
            .unwrap_or_default();
        Ok(self.word_labels(&model_words, &enc, &toks))
    }

    fn options(&self) -> PunctOptions {
        PunctOptions {
            unit: self.spec.unit.unwrap_or_default(),
            policy: self.spec.policy.unwrap_or_default(),
            threads: self.threads,
            marks_by_language: self.spec.marks_by_language.clone(),
            ..PunctOptions::default()
        }
    }

    fn take_stats(&mut self) -> LabelStats {
        std::mem::take(&mut self.stats)
    }
}

/// [`super::current`]: the punctuator for the selected punctuation file (else
/// the recommended one) of the data root at `paths`.
pub fn current(
    paths: &DataPaths,
    store: &dyn Store,
) -> std::result::Result<Option<Box<dyn Punctuator>>, Unavailable> {
    let manifest = Manifest::load(&paths.root).unwrap_or_else(|e| {
        log::warn!("punctuation: {e}; using the built-in model list");
        Manifest::builtin()
    });
    let file = match models::selected_file(store, ModelRole::Punctuation)? {
        Some(f) => f,
        None => {
            let files = store.list_model_files()?;
            match models::recommended(ModelRole::Punctuation, &files, &manifest) {
                Some(f) => f.clone(),
                None => return Ok(None),
            }
        }
    };
    let Some(spec) = file.capabilities.punctuation.clone() else {
        return Err(Unavailable::error(format!(
            "{} has no punctuation description; press Refresh in Settings",
            file.display_name
        )));
    };
    let entry = file
        .manifest_name
        .as_deref()
        .and_then(|n| manifest.model(n));
    // A model that lists no languages is not checked, as for speech models.
    let languages = if file.capabilities.languages.is_empty() {
        vec!["*".to_owned()]
    } else {
        file.capabilities.languages.clone()
    };
    let info = PunctModelInfo {
        name: file
            .manifest_name
            .clone()
            .unwrap_or_else(|| file.display_name.clone()),
        revision: entry.map(|e| e.revision.clone()).filter(|r| !r.is_empty()),
        sha256: file.sha256.clone(),
        languages,
        casing: spec.casing,
        runtime: PunctRuntime::default(),
    };
    let p = OnnxPunctuator::open(
        &paths.absolute(&file.path),
        spec,
        info,
        paths,
        super::threads(store),
    )?;
    Ok(Some(Box::new(p)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::punct::PunctReason;

    #[test]
    fn the_identity_check_names_both_paths() {
        let dir = tempfile::tempdir().unwrap();
        let ours = dir.path().join("diarist-onnxruntime.dll");
        let theirs = dir.path().join("onnxruntime.dll");
        std::fs::write(&ours, b"a").unwrap();
        std::fs::write(&theirs, b"b").unwrap();
        assert!(identity_check(&ours, &ours).is_ok());
        let e = identity_check(&ours, &theirs).unwrap_err();
        assert_eq!(e.reason, PunctReason::Runtime);
        assert!(
            e.message.contains(&ours.display().to_string())
                && e.message.contains(&theirs.display().to_string()),
            "{}",
            e.message
        );
        let v = check_version("1.17.260914", Some("1.31.0"), &ours).unwrap_err();
        assert_eq!(v.reason, PunctReason::Runtime);
        assert!(v.message.contains("1.17.260914") && v.message.contains("1.31.0"));
        assert!(check_version("1.31.0", Some("1.31"), &ours).is_ok());
        assert!(check_version("1.31.0", None, &ours).is_ok());
    }

    #[test]
    fn the_dependency_probe_finds_system_dlls_only() {
        assert!(system_dll_present("kernel32.dll"));
        assert!(!system_dll_present(
            "diarist-no-such-runtime-dependency.dll"
        ));
    }

    #[test]
    fn a_missing_library_is_a_runtime_failure() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(dir.path());
        let spec = PunctSpec::from_json(
            r#"{"tokenizer": "tokenizer.json", "max_tokens": 64,
                "heads": [{"output": "logits", "kind": "post", "labels": ["", "."],
                           "position": "last_token", "values": "logits"}]}"#,
        )
        .unwrap();
        let root = paths.models().join("m");
        std::fs::create_dir_all(&root).unwrap();
        let e = find_runtime(&root, &spec, &paths).unwrap_err();
        assert_eq!(e.reason, PunctReason::Runtime);
        assert!(
            e.message.contains("diarist-onnxruntime.dll"),
            "{}",
            e.message
        );
        // A shared copy under engine\onnxruntime-<version>\ is found second.
        let shared = paths.engine().join("onnxruntime-1.31.0").join("lib");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("onnxruntime.dll"), b"x").unwrap();
        assert_eq!(
            find_runtime(&root, &spec, &paths).unwrap(),
            shared.join("onnxruntime.dll")
        );
        std::fs::write(root.join("diarist-onnxruntime.dll"), b"x").unwrap();
        assert_eq!(
            find_runtime(&root, &spec, &paths).unwrap(),
            root.join("diarist-onnxruntime.dll")
        );
    }

    fn tok(chars: &[bool]) -> TokOut {
        TokOut {
            chars: Some(chars.to_vec()),
            ..TokOut::default()
        }
    }

    #[test]
    fn per_character_casing() {
        let p = |s: &[&str]| s.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            char_case(
                "paris",
                &p(&["par", "is"]),
                &[tok(&[true, false, false]), tok(&[false, false])]
            ),
            Case::First
        );
        assert_eq!(
            char_case(
                "nasa",
                &p(&["na", "sa"]),
                &[tok(&[true, true]), tok(&[true, true])]
            ),
            Case::Upper
        );
        assert_eq!(
            char_case(
                "iphone",
                &p(&["iph", "one"]),
                &[tok(&[false, true, false]), tok(&[false; 3])]
            ),
            Case::Chars(vec![false, true, false, false, false, false])
        );
        // The pieces do not spell the word (normalisation): first letter only.
        assert_eq!(
            char_case("ﬁne", &p(&["fine"]), &[tok(&[true, false, false, false])]),
            Case::First
        );
        assert_eq!(
            char_case("the", &p(&["the"]), &[tok(&[false; 3])]),
            Case::Keep
        );
    }

    /// Working set and peak working set of this process, bytes.
    #[cfg(windows)]
    pub(crate) fn rss() -> (u64, u64) {
        use windows::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        use windows::Win32::System::Threading::GetCurrentProcess;
        let mut c = PROCESS_MEMORY_COUNTERS {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            ..Default::default()
        };
        // SAFETY: a valid struct of the size given.
        unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) }.unwrap();
        (c.WorkingSetSize as u64, c.PeakWorkingSetSize as u64)
    }

    /// About 200 words of meeting talk the way the ASR writes them: lower
    /// case, no marks (written for this test).
    pub(crate) const FIXTURE: &str = "okay so let's get started thanks everyone for joining today \
        the plan is to go through the release first and then talk about the budget for next quarter \
        does that work for everyone great so the release is mostly done we fixed the import bug last week \
        and the new download screen went out on monday i think the only thing left is the installer \
        which still needs a signature from the vendor can you check on that tomorrow sure i'll send them \
        an email in the morning and copy you on it thanks now the budget we have about the same amount \
        as last year but the cloud costs went up so we need to cut something somewhere what do you think \
        we should drop honestly i would stop the second test machine we hardly use it and it costs more \
        than the licences for the design tools that makes sense to me anyone against it no okay then \
        let's do that and look at the numbers again in a month one more thing the office will be closed \
        on friday so please plan your deliveries around that is there anything else from anyone no \
        alright thank you all see you next week";

    /// The runner on a real model: `DIARIST_PUNCT_MODEL=<folder>` holding an
    /// `.onnx` with its `<file>.meet.json`, the tokenizer and
    /// `diarist-onnxruntime.dll` (dev only: the spike's candidate A plus the
    /// renamed ONNX Runtime 1.31.0). Run alone:
    /// `cargo test -p diarist-core --lib punct::onnx::tests::real_model -- --ignored --test-threads=1`.
    #[test]
    #[ignore = "needs DIARIST_PUNCT_MODEL (a punctuation model folder)"]
    #[cfg(windows)]
    fn real_model() {
        use crate::audio::echo::text::norm;
        let Some(folder) = std::env::var_os("DIARIST_PUNCT_MODEL").map(PathBuf::from) else {
            eprintln!("skipped: DIARIST_PUNCT_MODEL is not set");
            return;
        };
        let mut model = None;
        let mut stack = vec![folder.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "onnx")
                    && crate::models::Sidecar::path_for(&p).is_file()
                {
                    model = Some(p);
                }
            }
        }
        let model = model.expect("an .onnx with a .meet.json in DIARIST_PUNCT_MODEL");
        let sidecar = crate::models::Sidecar::read(&model).unwrap().unwrap();
        let spec = sidecar.punctuation.expect("a punctuation block");
        let root = tempfile::tempdir().unwrap();
        let paths = DataPaths::at(root.path()).with_models_dir(&folder);
        let words: Vec<&str> = FIXTURE.split_whitespace().collect();
        assert!((190..230).contains(&words.len()), "{}", words.len());
        let hint = LangHint {
            language: Some("en".into()),
            script: Script::Latin,
        };

        let (rss0, _) = rss();
        let t0 = std::time::Instant::now();
        let mut p = OnnxPunctuator::open(
            &model,
            spec,
            PunctModelInfo {
                name: "test".into(),
                languages: vec!["*".into()],
                ..PunctModelInfo::default()
            },
            &paths,
            4,
        )
        .unwrap();
        let load_s = t0.elapsed().as_secs_f64();
        // The module identity and version checks passed; the runtime is ours.
        let rt = loaded_runtime().unwrap();
        assert!(rt.path.ends_with(super::super::spec::RUNTIME_DLL), "{rt:?}");
        assert_eq!(p.descriptor().runtime.version, rt.version);
        assert!(rt.version.starts_with("1.31"), "{}", rt.version);

        let t1 = std::time::Instant::now();
        let a = p.label_stream(&words, &hint, &|| false).unwrap();
        let label_s = t1.elapsed().as_secs_f64();
        let b = p.label_stream(&words, &hint, &|| false).unwrap();
        assert_eq!(a.len(), words.len(), "one label per word");
        assert_eq!(a, b, "deterministic");
        let (_, peak) = rss();
        // Every label is a mark after the word, nothing that changes letters
        // (A has no casing head), and each marked word keeps its normalised
        // text (plan 16 section 3.3 rule 3).
        let marks = a.iter().filter(|l| l.post.is_some()).count();
        assert!(a.iter().all(|l| l.pre.is_none() && l.case == Case::Keep));
        let mut shown_text = Vec::new();
        for (w, l) in words.iter().zip(&a) {
            let shown = match l.post {
                Some(m) => format!(
                    "{w}{}",
                    match m {
                        super::super::Mark::Period => ".",
                        super::super::Mark::Comma => ",",
                        super::super::Mark::Question => "?",
                        super::super::Mark::Colon => ":",
                        super::super::Mark::Dash => "-",
                        _ => "!",
                    }
                ),
                None => (*w).to_owned(),
            };
            assert_eq!(norm(&shown), norm(w), "{w} -> {shown}");
            shown_text.push(shown);
        }
        eprintln!("{}", shown_text.join(" "));
        let rate = marks as f64 / words.len() as f64;
        assert!(
            (0.05..0.35).contains(&rate),
            "{marks} marks on {} words",
            words.len()
        );
        // A stream longer than one window: one label per word as well.
        let long: Vec<&str> = words
            .iter()
            .copied()
            .cycle()
            .take(words.len() * 6)
            .collect();
        assert_eq!(
            p.label_stream(&long, &hint, &|| false).unwrap().len(),
            long.len()
        );
        // Cancel is honoured.
        assert!(matches!(
            p.label_stream(&long, &hint, &|| true),
            Err(CoreError::Cancelled)
        ));

        // The identity check's failure path names both files, and another
        // library cannot be loaded beside ours.
        let other = PathBuf::from(r"C:\Windows\System32\onnxruntime.dll");
        let e = identity_check(&rt.path, &other).unwrap_err();
        assert_eq!(e.reason, PunctReason::Runtime);
        assert!(e.message.contains("System32") && e.message.contains(RUNTIME_DLL_NAME));
        if other.is_file() {
            let e = ensure_runtime(&other, None).unwrap_err();
            assert_eq!(e.reason, PunctReason::Runtime);
            assert!(e.message.contains("restart"), "{}", e.message);
        }

        drop(p);
        let (rss1, _) = rss();
        let mb = |b: u64| b as f64 / 1_048_576.0;
        eprintln!(
            "load {load_s:.2} s, 200 words {label_s:.3} s, {marks} marks, RSS start {:.0} MB, peak {:.0} MB, after drop {:.0} MB",
            mb(rss0),
            mb(peak),
            mb(rss1)
        );
        assert!(
            rss1 < rss0 + 100 * 1_048_576,
            "RSS after the session is dropped: {:.0} MB, at the start {:.0} MB",
            mb(rss1),
            mb(rss0)
        );
    }

    const RUNTIME_DLL_NAME: &str = super::super::spec::RUNTIME_DLL;
}
