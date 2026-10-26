//! The ONNX Runtime punctuator (docs/plans/16 sections 3.6 and 3.7): one
//! generic runner for any token classifier its description block
//! ([`PunctSpec`]) describes, on the CPU execution provider only.
//!
//! Loading, once per process (ort's environment is process-global, so the
//! first library loaded is the one every session uses; another model folder
//! whose block names the same minor release uses the loaded library, one
//! that needs another release takes effect after a restart):
//!
//! 1. The library is the model folder's `diarist-onnxruntime.dll` (renamed,
//!    so the Windows loader never confuses it with another copy, such as
//!    System32's 1.17), else `engine\onnxruntime-<version>\diarist-onnxruntime.dll`,
//!    as an absolute path (a relative data root would otherwise be looked
//!    up next to the exe first).
//! 2. Before anything is loaded: when the model came from the manifest and
//!    the entry lists the library, its SHA-256 must match; its Visual C++
//!    runtime dependencies must be in System32 (or next to it), else the
//!    load is refused naming the missing DLL and Microsoft's page.
//! 3. The library is preloaded with `LoadLibraryExW(path,
//!    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32)`, so
//!    its dependencies resolve only from its own folder or System32 (never
//!    the current directory or `PATH`); `ort::init_from(<absolute path>)`
//!    then loads the same full path, which reuses that module (`api-31`: an
//!    older library makes the load fail with ort's own message). A long
//!    path (a deep data folder) goes to both loads in the extended form
//!    (`\\?\`, `process::path_arg`); the loader refuses the plain form past
//!    about 250 characters.
//! 4. Module identity: the module that holds the API's functions must be
//!    the file we loaded (`GetModuleHandleExW` from a function address,
//!    then `GetModuleFileNameW`), and its version must have the major and
//!    minor version of the block's `runtime_version` (any patch).
//!
//! Any failure gives [`Unavailable::runtime`], so the run records
//! `reason: "runtime"` and Diagnostics shows the "Punctuation runtime" row.
//! The session is created per punctuator and dropped with it.

use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use ort::environment::Environment;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::{Session, SessionOutputs};
use ort::value::Tensor;

use super::spec::{
    major_minor, parse_case_label, rel_path, HeadKind, HeadPosition, HeadValues, InputCasing,
    MarkLabel, PunctHead, PunctSpec, RUNTIME_DLL,
};
use super::tokenize::HfTokenizer;
use super::windows::{run_windows, Encoded, TokenCounter, WindowOpts};
use super::{
    Case, LabelStats, LangHint, PunctModelInfo, PunctOptions, PunctRuntime, Punctuator, Script,
    Unavailable, WordLabel,
};
use crate::error::{CoreError, Result};
use crate::models::manifest::normalized_relative;
use crate::models::{self, DataPaths, Manifest, ModelEntry};
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
    /// Absolute.
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

/// What this process did with ONNX Runtime so far (Diagnostics).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeStatus {
    /// Nothing loaded yet (it loads at the first punctuated run).
    NotLoaded,
    Ready(LoadedRuntime),
    /// Loaded, but a check failed; punctuation is off until a restart.
    Broken {
        path: PathBuf,
        message: String,
    },
}

/// [`RuntimeStatus`] now.
pub fn runtime_status() -> RuntimeStatus {
    match &*STATE.lock().unwrap_or_else(|p| p.into_inner()) {
        None => RuntimeStatus::NotLoaded,
        Some(Attempt::Ready { runtime, .. }) => RuntimeStatus::Ready(runtime.clone()),
        Some(Attempt::Broken { path, message }) => RuntimeStatus::Broken {
            path: path.clone(),
            message: message.clone(),
        },
    }
}

/// What a model asks of the library.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeWant {
    /// The block's `runtime_version`: the major and minor version must be
    /// the loaded library's (any patch).
    pub version: Option<String>,
    /// The SHA-256 the manifest entry lists for the library, when the model
    /// file came from the manifest and the library is its folder's own
    /// ([`manifest_runtime_sha`]); `None` for a drop-in or a shared copy.
    pub sha256: Option<String>,
}

/// A model's folder, its library (absolute) and what the library must be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePlan {
    pub root: PathBuf,
    pub dll: PathBuf,
    pub want: RuntimeWant,
}

/// The folder, library and checks for the `.onnx` file at `model` with
/// the block `spec`; `entry` is the manifest entry the file matched (by
/// name, size and hash), if any.
pub fn plan_runtime(
    model: &Path,
    spec: &PunctSpec,
    paths: &DataPaths,
    entry: Option<&ModelEntry>,
) -> std::result::Result<RuntimePlan, Unavailable> {
    let root = spec
        .model_root(model, &paths.models())
        .map_err(|e| model_err("folder", e))?;
    let dll = find_runtime(&root, spec, paths)?;
    let sha256 = manifest_runtime_sha(entry, spec, &root, &dll);
    Ok(RuntimePlan {
        want: RuntimeWant {
            version: spec.runtime_version.clone(),
            sha256,
        },
        root,
        dll,
    })
}

/// The SHA-256 the manifest lists for the library at `dll`: only when the
/// model file matched a manifest `entry`, `dll` is the model folder's own
/// library (the block's `runtime` under `root`, not a shared copy under
/// `engine\`), and the entry lists that file among its `files`. A drop-in's
/// library is not checked (its folder is the user's, docs/04 section 5).
pub fn manifest_runtime_sha(
    entry: Option<&ModelEntry>,
    spec: &PunctSpec,
    root: &Path,
    dll: &Path,
) -> Option<String> {
    let entry = entry?;
    if !same_file(&absolute(&rel_path(root, spec.runtime_file())), dll) {
        return None;
    }
    let want = normalized_relative(spec.runtime_file());
    entry
        .files
        .iter()
        .find(|f| normalized_relative(&f.path) == want)
        .map(|f| f.sha256.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Loads (once) and checks the library at `dll`; see the module docs.
pub fn ensure_runtime(
    dll: &Path,
    want: &RuntimeWant,
) -> std::result::Result<(LoadedRuntime, Environment), Unavailable> {
    ensure_runtime_with(dll, want, &missing_dependency)
}

/// [`ensure_runtime`] with the dependency probe given (tests inject one).
fn ensure_runtime_with(
    dll: &Path,
    want: &RuntimeWant,
    probe: &dyn Fn(&Path) -> Option<&'static str>,
) -> std::result::Result<(LoadedRuntime, Environment), Unavailable> {
    let dll = absolute(dll);
    let mut state = STATE.lock().unwrap_or_else(|p| p.into_inner());
    match &*state {
        Some(Attempt::Ready { runtime, env }) => {
            reuse_loaded(runtime, &dll, want.version.as_deref())?;
            return Ok((runtime.clone(), env.clone()));
        }
        Some(Attempt::Broken { path, message }) => {
            return Err(Unavailable::runtime(if same_file(path, &dll) {
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
    before_load(&dll, want.sha256.as_deref(), probe)?;
    // The loader refuses a library path of more than about 250 characters
    // in the plain form (chunk D measured 251 failing): both loads get the
    // extended form (`\\?\`) of a long path, the same string, so ort's load
    // finds the preloaded module. `dll` stays plain in what is recorded
    // and shown, and the identity check compares files, not spellings.
    let load_path = PathBuf::from(crate::engine::process::path_arg(&dll));
    let preloaded = preload(&load_path, &dll)?;
    let init = ort::init_from(&load_path);
    // ort's load of the same full path took its own reference to the
    // preloaded module; after a failed init this unloads it, so a later
    // call may retry.
    drop(preloaded);
    let builder = match init {
        Ok(b) => b,
        Err(e) => {
            let e = e
                .to_string()
                .replace(&load_path.display().to_string(), &dll.display().to_string());
            let u = Unavailable::runtime(format!("cannot load {}: {e}", dll.display()));
            log::error!("punctuation: {}", u.message);
            return Err(u);
        }
    };
    let checked = (|| {
        let module = api_module_path()?;
        identity_check(&dll, &module)?;
        let version = api_version()?;
        let env = builder
            .with_name("diarist-punctuation")
            .with_telemetry(false)
            .build()
            .map_err(|e| Unavailable::runtime(format!("ONNX Runtime environment: {e}")))?;
        Ok::<_, Unavailable>((
            LoadedRuntime {
                path: dll.clone(),
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
            // The library is ours and works; whether this model may use it
            // is its block's business (another model may still).
            check_version(&runtime.version, want.version.as_deref(), &dll)?;
            Ok((runtime, env))
        }
        Err(u) => {
            log::error!("punctuation: {}", u.message);
            *state = Some(Attempt::Broken {
                path: dll,
                message: u.message.clone(),
            });
            Err(u)
        }
    }
}

/// Whether a model whose library is `dll` and whose block names `want` can
/// use the library already `loaded` in this process: any folder's copy of
/// the same minor release can (ort's environment is process-wide, so the
/// loaded module serves it); another release needs a restart.
pub fn reuse_loaded(
    loaded: &LoadedRuntime,
    dll: &Path,
    want: Option<&str>,
) -> std::result::Result<(), Unavailable> {
    let same = same_file(&loaded.path, dll);
    match check_version(&loaded.version, want, &loaded.path) {
        Ok(()) => {
            if !same {
                log::info!(
                    "punctuation: {} uses ONNX Runtime {} already loaded from {}",
                    dll.display(),
                    loaded.version,
                    loaded.path.display()
                );
            }
            Ok(())
        }
        Err(e) if same => Err(e),
        Err(_) => Err(Unavailable::runtime(format!(
            "ONNX Runtime {} is already loaded from {}, and the model needs {} ({}): restart \
             Diarist to use it",
            loaded.version,
            loaded.path.display(),
            wanted_release(want.unwrap_or_default()),
            dll.display()
        ))),
    }
}

/// The checks before anything is loaded: the file is there; when the
/// manifest lists its hash (`want_sha256`), it is that file; and its Visual
/// C++ runtime is installed (`probe`, [`missing_dependency`] outside tests).
/// A missing dependency refuses the load (plan 16 section 3.7): loading
/// would fail with a vague error, or take a copy from elsewhere.
pub fn before_load(
    dll: &Path,
    want_sha256: Option<&str>,
    probe: &dyn Fn(&Path) -> Option<&'static str>,
) -> std::result::Result<(), Unavailable> {
    if !dll.is_file() {
        return Err(Unavailable::runtime(format!(
            "the ONNX Runtime library {} is missing",
            dll.display()
        )));
    }
    if let Some(want) = want_sha256.map(str::trim).filter(|s| !s.is_empty()) {
        let got = models::download::sha256_file(dll)
            .map_err(|e| Unavailable::runtime(format!("cannot read {}: {e}", dll.display())))?;
        if !got.eq_ignore_ascii_case(want) {
            return Err(Unavailable::runtime(format!(
                "{} has SHA-256 {got}, the manifest lists {want}: download the punctuation \
                 model again (Settings, Models)",
                dll.display()
            )));
        }
    }
    if let Some(d) = probe(dll) {
        return Err(Unavailable::runtime(missing_dependency_message(d)));
    }
    Ok(())
}

/// What to do about a missing Visual C++ runtime DLL `d`.
pub fn missing_dependency_message(d: &str) -> String {
    format!(
        "{d} is missing: ONNX Runtime needs the Microsoft Visual C++ Redistributable (x64). \
         Install it from {VC_REDIST_URL}, then restart Diarist"
    )
}

/// `path` made absolute from the current directory (`GetFullPathNameW`,
/// no `\\?\` prefix), as it is.
fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
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

/// Whether the library version `version` serves a block that names `want`:
/// same major and minor version, any patch (`1.31.0` and `1.31` accept
/// 1.31.2, not 1.32.0). No `want`: any.
pub fn version_matches(version: &str, want: Option<&str>) -> bool {
    match want.map(str::trim).filter(|w| !w.is_empty()) {
        None => true,
        Some(w) => major_minor(w).is_some_and(|need| major_minor(version) == Some(need)),
    }
}

/// `1.31.x` for `1.31.0`, the text as it is otherwise.
fn wanted_release(want: &str) -> String {
    match major_minor(want) {
        Some((a, b)) => format!("{a}.{b}.x"),
        None => want.trim().to_owned(),
    }
}

/// The version check ([`version_matches`]); the message names both.
pub fn check_version(
    version: &str,
    want: Option<&str>,
    dll: &Path,
) -> std::result::Result<(), Unavailable> {
    if version_matches(version, want) {
        Ok(())
    } else {
        Err(Unavailable::runtime(format!(
            "{} is ONNX Runtime {version}, the model needs {}",
            dll.display(),
            wanted_release(want.unwrap_or_default())
        )))
    }
}

/// Whether `a` and `b` name the same file: both canonicalised when they
/// exist, else their absolute forms; either way without the extended-length
/// prefix (`\\?\C:\x` is `C:\x`, `\\?\UNC\s\x` is `\\s\x`), since a module
/// loaded by its long path reports that form.
fn same_file(a: &Path, b: &Path) -> bool {
    let plain = |p: PathBuf| {
        let s = p.to_string_lossy().into_owned();
        match s.strip_prefix(r"\\?\UNC\") {
            Some(rest) => format!(r"\\{rest}"),
            None => s.strip_prefix(r"\\?\").map(str::to_owned).unwrap_or(s),
        }
    };
    let (x, y) = match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => (x, y),
        _ => (absolute(a), absolute(b)),
    };
    plain(x).eq_ignore_ascii_case(&plain(y))
}

/// The first of [`RUNTIME_DEPENDENCIES`] that is neither in System32 nor
/// next to the library at `dll` (the two places the preload resolves the
/// library's imports from).
pub fn missing_dependency(dll: &Path) -> Option<&'static str> {
    let own = dll.parent();
    RUNTIME_DEPENDENCIES
        .iter()
        .copied()
        .find(|d| !system_dll_present(d) && !own.is_some_and(|o| o.join(d).is_file()))
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

/// A reference on the library, taken before ort loads it, so its
/// dependencies resolve from its own folder and System32 only; released on
/// drop.
#[cfg(windows)]
struct Preloaded(windows::Win32::Foundation::HMODULE);

#[cfg(windows)]
impl Drop for Preloaded {
    fn drop(&mut self) {
        // SAFETY: a module this process loaded and still references.
        unsafe {
            let _ = windows::Win32::Foundation::FreeLibrary(self.0);
        }
    }
}

/// `LoadLibraryExW(load, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR |
/// LOAD_LIBRARY_SEARCH_SYSTEM32)` (`load` absolute: `dll`, in the extended
/// form when long), with the system's error boxes off for the call; a
/// failure names `dll`.
#[cfg(windows)]
fn preload(load: &Path, dll: &Path) -> std::result::Result<Preloaded, Unavailable> {
    use windows::core::HSTRING;
    use windows::Win32::System::Diagnostics::Debug::{
        SetThreadErrorMode, SEM_FAILCRITICALERRORS, THREAD_ERROR_MODE,
    };
    use windows::Win32::System::LibraryLoader::{
        LoadLibraryExW, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
    };
    let mut old = THREAD_ERROR_MODE::default();
    // SAFETY: plain Win32 calls with valid arguments; the error mode is
    // restored on this thread right after the load.
    let loaded = unsafe {
        let _ = SetThreadErrorMode(SEM_FAILCRITICALERRORS, Some(&mut old));
        let r = LoadLibraryExW(
            &HSTRING::from(load.as_os_str()),
            None,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        );
        let _ = SetThreadErrorMode(old, None);
        r
    };
    loaded.map(Preloaded).map_err(|e| {
        Unavailable::runtime(format!(
            "cannot load {}: {e} (its dependencies are looked for in its own folder and in \
             System32 only)",
            dll.display()
        ))
    })
}

#[cfg(not(windows))]
fn preload(_load: &Path, _dll: &Path) -> std::result::Result<(), Unavailable> {
    Ok(())
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

/// The library for a model folder, as an absolute path: the block's
/// `runtime` file in the folder (default `diarist-onnxruntime.dll`), else a
/// shared copy `engine\onnxruntime-<version>\diarist-onnxruntime.dll` (a
/// folder whose version has the block's major and minor version first,
/// newest first). Never a file named `onnxruntime.dll`. `Err` lists where
/// it looked.
pub fn find_runtime(
    root: &Path,
    spec: &PunctSpec,
    paths: &DataPaths,
) -> std::result::Result<PathBuf, Unavailable> {
    let own = absolute(&rel_path(root, spec.runtime_file()));
    if own.is_file() {
        return Ok(own);
    }
    let engine = absolute(&paths.engine());
    let folder_version = |name: &str| -> String {
        name.get("onnxruntime".len()..)
            .unwrap_or_default()
            .trim_start_matches(['-', '_', ' '])
            .to_owned()
    };
    let mut dirs: Vec<(String, PathBuf)> = std::fs::read_dir(&engine)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .filter_map(|p| {
                    let name = p.file_name()?.to_str()?.to_owned();
                    name.to_ascii_lowercase()
                        .starts_with("onnxruntime")
                        .then(|| (folder_version(&name), p))
                })
                .collect()
        })
        .unwrap_or_default();
    let want = spec.runtime_version.as_deref();
    dirs.sort_by(|(a, _), (b, _)| {
        version_matches(b, want)
            .cmp(&version_matches(a, want))
            .then_with(|| models::manifest::version_cmp(b, a))
    });
    for (_, d) in &dirs {
        let p = d.join(RUNTIME_DLL);
        if p.is_file() {
            return Ok(p);
        }
    }
    Err(Unavailable::runtime(format!(
        "the ONNX Runtime library is missing: not at {} nor at {}",
        own.display(),
        engine
            .join("onnxruntime-<version>")
            .join(RUNTIME_DLL)
            .display()
    )))
}

/// Where the library is, as `engine_info.punctuation.runtime.path` records
/// it: relative to the data root (`models/<folder>/diarist-onnxruntime.dll`),
/// else `<models>/...` or `<engine>/...` relative to the models or engine
/// folder when that folder is outside the root (`--models-dir`,
/// `--engine-dir`), else `<elsewhere>/<file name>`. Never absolute, so no
/// profile path reaches the database.
pub fn recorded_path(paths: &DataPaths, dll: &Path) -> String {
    let under = |dir: &Path| -> Option<String> {
        let (d, f) = (absolute(dir), absolute(dll));
        let rel = match f.strip_prefix(&d) {
            Ok(r) => r.to_path_buf(),
            Err(_) => {
                let (d, f) = (
                    std::fs::canonicalize(&d).ok()?,
                    std::fs::canonicalize(&f).ok()?,
                );
                f.strip_prefix(&d).ok()?.to_path_buf()
            }
        };
        let parts: Vec<String> = rel
            .components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();
        (!parts.is_empty()).then(|| parts.join("/"))
    };
    if let Some(r) = under(&paths.root) {
        return r;
    }
    if let Some(r) = under(&paths.models()) {
        return format!("<models>/{r}");
    }
    if let Some(r) = under(&paths.engine()) {
        return format!("<engine>/{r}");
    }
    format!(
        "<elsewhere>/{}",
        dll.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    )
}

/// `message` with the data root, the models folder and the engine folder
/// written `<data root>`, `<models>` and `<engine>`, and anything else under
/// the user's profile as `~` ([`crate::diagnostics::scrub_profile`]): what
/// `engine_info.punctuation.message` may hold (no profile path in the
/// database). The log keeps the full paths.
pub fn scrub_message(paths: &DataPaths, message: &str) -> String {
    let mut dirs = [
        (absolute(&paths.models()), "<models>"),
        (absolute(&paths.engine()), "<engine>"),
        (absolute(&paths.root), "<data root>"),
    ];
    // A nested folder first, so `<models>` wins over `<data root>\models`.
    dirs.sort_by_key(|(d, _)| std::cmp::Reverse(d.as_os_str().len()));
    let mut out = message.to_owned();
    for (d, label) in &dirs {
        // Never a bare drive or root.
        if d.components().any(|c| matches!(c, Component::Normal(_))) {
            out = out.replace(&d.display().to_string(), label);
        }
    }
    crate::diagnostics::scrub_profile(&out)
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

/// The heads the runner reads (a `boundary` head is read by nothing).
struct Heads {
    post: Option<Head>,
    pre: Option<Head>,
    case: Option<Head>,
}

impl Heads {
    fn of(spec: &PunctSpec) -> Self {
        let head = |k: HeadKind| spec.head(k).map(Head::new);
        Self {
            post: head(HeadKind::Post),
            pre: head(HeadKind::Pre),
            case: head(HeadKind::Case),
        }
    }

    /// The word labels from the tokens' outputs; marks predicted inside a
    /// word are counted in `stats`.
    fn word_labels(
        &self,
        stats: &mut LabelStats,
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
                    stats.inner_marks_dropped += inner as u64;
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
                    stats.inner_marks_dropped += 1;
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
                    char_case(
                        &model_words[w],
                        &enc.pieces[r.clone()],
                        &enc.lead[r.clone()],
                        &toks[r.clone()],
                    )
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

/// A punctuation model in ONNX Runtime.
pub struct OnnxPunctuator {
    session: Session,
    tokenizer: HfTokenizer,
    spec: PunctSpec,
    info: PunctModelInfo,
    threads: u32,
    heads: Heads,
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
    /// for the `.onnx` file at `model`; `entry` is the manifest entry the
    /// file matched, if any (its hash of the library is checked).
    /// `info.runtime` is filled in, its path never absolute.
    pub fn open(
        model: &Path,
        spec: PunctSpec,
        mut info: PunctModelInfo,
        paths: &DataPaths,
        entry: Option<&ModelEntry>,
        threads: u32,
    ) -> std::result::Result<Self, Unavailable> {
        spec.validate().map_err(|e| model_err("description", e))?;
        let plan = plan_runtime(model, &spec, paths, entry)?;
        let (runtime, env) = ensure_runtime(&plan.dll, &plan.want)?;
        info.runtime = PunctRuntime {
            name: "onnxruntime".into(),
            version: runtime.version.clone(),
            path: recorded_path(paths, &runtime.path),
        };
        let root = plan.root;
        let tokenizer = HfTokenizer::load(&rel_path(&root, &spec.tokenizer))
            .map_err(|e| model_err("tokenizer", e))?;
        let threads = threads.max(1);
        let opt_err =
            |e: ort::Error<ort::session::builder::SessionBuilder>| model_err("session options", e);
        // ONNX Runtime opens the model itself and, like the loader, refuses
        // the plain form of a long path: the extended form when long, the
        // plain one in messages.
        let load_model = PathBuf::from(crate::engine::process::path_arg(model));
        let session = Session::builder(&env)
            .map_err(|e| model_err("session", e))?
            .with_optimization_level(GraphOptimizationLevel::All)
            .map_err(opt_err)?
            .with_intra_threads(threads as usize)
            .map_err(opt_err)?
            .with_inter_threads(1)
            .map_err(opt_err)?
            .commit_from_file(&load_model)
            .map_err(|e| {
                let plain = model.display().to_string();
                let e = e
                    .to_string()
                    .replace(&load_model.display().to_string(), &plain);
                model_err(&plain, e)
            })?;
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
        Ok(Self {
            heads: Heads::of(&spec),
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
        let heads = [&self.heads.post, &self.heads.pre, &self.heads.case];
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
            for (r, row) in out.iter_mut().enumerate() {
                for (p, o) in row.iter_mut().enumerate() {
                    let at = r * t + off + p;
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
}

/// Per-character casing of one word from its tokens' flags: one flag per
/// character when the pieces spell the word as the model saw it, else the
/// first letter's flag only (SentencePiece's normalisation can change the
/// characters, plan 16 section 3.3 rule 6). A token's flags count its
/// stripped `▁` markers (`lead`) as characters, so letter `i` of a piece has
/// flag `lead + i` (the spike's `punct.py` starts at index 1 for a piece
/// that begins with `▁`).
fn char_case(model_word: &str, pieces: &[String], lead: &[usize], toks: &[TokOut]) -> Case {
    let mut spelled = String::new();
    let mut flags = Vec::new();
    for ((piece, &lead), t) in pieces.iter().zip(lead).zip(toks) {
        let f = t.chars.as_deref().unwrap_or(&[]);
        for (i, ch) in piece.chars().enumerate() {
            spelled.push(ch);
            flags.push(f.get(lead + i).copied().unwrap_or(false));
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
        // One stream per call: the mic and loopback streams of a run are
        // not batched together yet (`run_windows` takes several streams;
        // the `Punctuator` trait gives one at a time). Later, review note 6.
        let toks = run_windows(&[&enc], &opts, cancel, &mut |batch| self.infer(batch))?
            .pop()
            .unwrap_or_default();
        Ok(self
            .heads
            .word_labels(&mut self.stats, &model_words, &enc, &toks))
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
/// the recommended one) of the data root at `paths`. A failure's message
/// goes into `engine_info.punctuation.message`, so its paths are written
/// relative to the data folders ([`scrub_message`]); the log keeps them
/// whole.
pub fn current(
    paths: &DataPaths,
    store: &dyn Store,
) -> std::result::Result<Option<Box<dyn Punctuator>>, Unavailable> {
    current_unscrubbed(paths, store).map_err(|u| {
        log::info!("punctuation: {}", u.message);
        Unavailable {
            reason: u.reason,
            message: scrub_message(paths, &u.message),
        }
    })
}

fn current_unscrubbed(
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
        entry,
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
        assert!(
            v.message.contains("1.17.260914") && v.message.contains("1.31.x"),
            "{}",
            v.message
        );
        assert!(check_version("1.31.0", Some("1.31"), &ours).is_ok());
        assert!(check_version("1.31.0", None, &ours).is_ok());
    }

    /// `runtime_version` is a minor release: any patch of 1.31 serves a
    /// block that names `1.31.0` or `1.31`; 1.30 and 1.32 do not (docs/04).
    #[test]
    fn the_runtime_version_matches_by_major_and_minor() {
        for (have, want) in [
            ("1.31.0", "1.31.0"),
            ("1.31.2", "1.31.0"),
            ("1.31.0", "1.31"),
            ("1.31.1", " 1.31.5 "),
        ] {
            assert!(version_matches(have, Some(want)), "{have} for {want}");
        }
        for (have, want) in [
            ("1.30.9", "1.31.0"),
            ("1.32.0", "1.31.0"),
            ("2.31.0", "1.31.0"),
            ("1.3.1", "1.31"),
            ("garbage", "1.31"),
        ] {
            assert!(!version_matches(have, Some(want)), "{have} for {want}");
        }
        assert!(version_matches("1.17.0", None) && version_matches("1.17.0", Some("")));
    }

    /// A second model folder with the same minor release uses the library
    /// already loaded; another release asks for a restart (with both paths).
    #[test]
    fn a_loaded_library_serves_another_folder_of_the_same_release() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.dll"), dir.path().join("b.dll"));
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(&b, b"b").unwrap();
        let loaded = LoadedRuntime {
            path: a.clone(),
            version: "1.31.0".into(),
        };
        assert!(reuse_loaded(&loaded, &a, Some("1.31.0")).is_ok());
        assert!(reuse_loaded(&loaded, &b, Some("1.31.0")).is_ok());
        assert!(reuse_loaded(&loaded, &b, Some("1.31")).is_ok());
        assert!(reuse_loaded(&loaded, &b, None).is_ok());
        let e = reuse_loaded(&loaded, &b, Some("1.32.0")).unwrap_err();
        assert_eq!(e.reason, PunctReason::Runtime);
        assert!(
            e.message.contains("restart")
                && e.message.contains("1.32.x")
                && e.message.contains(&a.display().to_string())
                && e.message.contains(&b.display().to_string()),
            "{}",
            e.message
        );
        // The same file with the wrong release: a restart would not help.
        let e = reuse_loaded(&loaded, &a, Some("1.30")).unwrap_err();
        assert!(!e.message.contains("restart"), "{}", e.message);
    }

    /// A missing Visual C++ runtime refuses the load before anything is
    /// loaded, naming the DLL and Microsoft's page; a wrong hash refuses it
    /// too.
    #[test]
    fn a_missing_dependency_or_a_wrong_hash_refuses_the_load() {
        let dir = tempfile::tempdir().unwrap();
        let dll = dir.path().join(RUNTIME_DLL);
        std::fs::write(&dll, b"MZ not really a library").unwrap();
        let none = |_: &Path| -> Option<&'static str> { None };
        let msvcp = |_: &Path| -> Option<&'static str> { Some("MSVCP140.dll") };
        assert!(before_load(&dll, None, &none).is_ok());
        let e = before_load(&dll, None, &msvcp).unwrap_err();
        assert_eq!(e.reason, PunctReason::Runtime);
        assert!(
            e.message.contains("MSVCP140.dll") && e.message.contains(VC_REDIST_URL),
            "{}",
            e.message
        );
        // Through ensure_runtime: refused, nothing loaded (when no other
        // test of this process loaded a library already).
        if runtime_status() == RuntimeStatus::NotLoaded {
            let e = ensure_runtime_with(&dll, &RuntimeWant::default(), &msvcp).unwrap_err();
            assert!(e.message.contains("MSVCP140.dll"), "{}", e.message);
            assert_eq!(runtime_status(), RuntimeStatus::NotLoaded);
        }
        let right = models::download::sha256_file(&dll).unwrap();
        assert!(before_load(&dll, Some(&right.to_uppercase()), &none).is_ok());
        let e = before_load(&dll, Some(&"0".repeat(64)), &none).unwrap_err();
        assert_eq!(e.reason, PunctReason::Runtime);
        assert!(
            e.message.contains("SHA-256") && e.message.contains(&right),
            "{}",
            e.message
        );
        let e = before_load(&dir.path().join("gone.dll"), None, &none).unwrap_err();
        assert!(e.message.contains("missing"), "{}", e.message);
        // A dependency next to the library counts (the preload looks there).
        if missing_dependency(&dll).is_some() {
            for d in RUNTIME_DEPENDENCIES {
                std::fs::write(dir.path().join(d), b"x").unwrap();
            }
        }
        assert_eq!(missing_dependency(&dll), None);
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
        // A file named onnxruntime.dll is never taken, in the model's
        // folder or a shared one.
        let shared = paths.engine().join("onnxruntime-1.31.0");
        std::fs::create_dir_all(shared.join("lib")).unwrap();
        for p in [
            root.join("onnxruntime.dll"),
            shared.join("onnxruntime.dll"),
            shared.join("lib").join("onnxruntime.dll"),
        ] {
            std::fs::write(p, b"x").unwrap();
        }
        assert!(find_runtime(&root, &spec, &paths).is_err());
        // A shared diarist-onnxruntime.dll under engine\onnxruntime-<version>\
        // is found second: the block's minor release first, newest first.
        let with_version = |v: &str| {
            PunctSpec::from_json(&format!(
                r#"{{"tokenizer": "tokenizer.json", "max_tokens": 64, "runtime_version": "{v}",
                    "heads": [{{"output": "logits", "kind": "post", "labels": ["", "."],
                               "position": "last_token", "values": "logits"}}]}}"#
            ))
            .unwrap()
        };
        for v in ["1.31.0", "1.31.2", "1.32.0"] {
            let d = paths.engine().join(format!("onnxruntime-{v}"));
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(RUNTIME_DLL), b"x").unwrap();
        }
        let found = |s: &PunctSpec| {
            let p = find_runtime(&root, s, &paths).unwrap();
            assert!(p.is_absolute(), "{}", p.display());
            p.parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        };
        assert_eq!(found(&with_version("1.31.0")), "onnxruntime-1.31.2");
        assert_eq!(found(&with_version("1.32")), "onnxruntime-1.32.0");
        assert_eq!(found(&spec), "onnxruntime-1.32.0", "no version: newest");
        std::fs::write(root.join(RUNTIME_DLL), b"x").unwrap();
        assert!(same_file(
            &find_runtime(&root, &spec, &paths).unwrap(),
            &root.join(RUNTIME_DLL)
        ));
    }

    /// Path from `base` to `target` with `..` (same drive), for a data root
    /// given relative to the current directory.
    fn relative_to(base: &Path, target: &Path) -> Option<PathBuf> {
        let (b, t): (Vec<_>, Vec<_>) = (base.components().collect(), target.components().collect());
        if b.first() != t.first() {
            return None;
        }
        let common = b.iter().zip(&t).take_while(|(x, y)| x == y).count();
        let mut out = PathBuf::new();
        for _ in common..b.len() {
            out.push("..");
        }
        for c in &t[common..] {
            out.push(c.as_os_str());
        }
        Some(out)
    }

    /// With a relative data root (`--data-dir`, `DIARIST_DATA_DIR`), the
    /// library path is made absolute before it is checked and loaded (ort
    /// would try `<exe dir>\<relative>` first), and it is recorded relative
    /// to the data root, never absolute.
    #[test]
    fn a_relative_data_root_gives_an_absolute_library_path() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = std::env::current_dir().unwrap();
        let Some(rel) = relative_to(&cwd, dir.path()) else {
            eprintln!("skipped: the temp folder is on another drive");
            return;
        };
        assert!(rel.is_relative());
        let paths = DataPaths::at(&rel);
        let spec = PunctSpec::from_json(
            r#"{"tokenizer": "tokenizer.json", "max_tokens": 64,
                "heads": [{"output": "logits", "kind": "post", "labels": ["", "."],
                           "position": "last_token", "values": "logits"}]}"#,
        )
        .unwrap();
        let root = paths.models().join("m");
        assert!(root.is_relative());
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("tokenizer.json"), b"{}").unwrap();
        std::fs::write(root.join(RUNTIME_DLL), b"MZ").unwrap();
        let plan = plan_runtime(&root.join("m.onnx"), &spec, &paths, None).unwrap();
        assert!(plan.dll.is_absolute(), "{}", plan.dll.display());
        assert!(same_file(
            &plan.dll,
            &dir.path().join("models").join("m").join(RUNTIME_DLL)
        ));
        assert_eq!(
            recorded_path(&paths, &plan.dll),
            "models/m/diarist-onnxruntime.dll"
        );
        // ensure_runtime checks the absolute form (a refused probe names it).
        if runtime_status() == RuntimeStatus::NotLoaded {
            let e = ensure_runtime_with(&root.join(RUNTIME_DLL), &RuntimeWant::default(), &|_| {
                Some("VCRUNTIME140.dll")
            })
            .unwrap_err();
            assert!(e.message.contains("VCRUNTIME140.dll"), "{}", e.message);
            let e = ensure_runtime_with(&root.join("gone.dll"), &RuntimeWant::default(), &|_| None)
                .unwrap_err();
            assert!(
                e.message.contains(&absolute(&root).display().to_string()),
                "{}",
                e.message
            );
        }
    }

    /// `engine_info.punctuation.runtime.path` and `.message` hold no
    /// absolute path of the data folders, wherever they are.
    #[test]
    fn recorded_paths_and_messages_are_never_absolute() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let elsewhere = dir.path().join("elsewhere");
        let paths = DataPaths::at(&root)
            .with_models_dir(elsewhere.join("models"))
            .with_engine_dir(elsewhere.join("engine"));
        let in_models = elsewhere.join("models").join("p").join(RUNTIME_DLL);
        let in_engine = elsewhere
            .join("engine")
            .join("onnxruntime-1.31.0")
            .join(RUNTIME_DLL);
        assert_eq!(
            recorded_path(&paths, &in_models),
            "<models>/p/diarist-onnxruntime.dll"
        );
        assert_eq!(
            recorded_path(&paths, &in_engine),
            "<engine>/onnxruntime-1.31.0/diarist-onnxruntime.dll"
        );
        assert_eq!(
            recorded_path(&paths, &dir.path().join("x").join(RUNTIME_DLL)),
            "<elsewhere>/diarist-onnxruntime.dll"
        );
        assert_eq!(
            recorded_path(
                &DataPaths::at(&root),
                &root.join("models").join("p").join("a.dll")
            ),
            "models/p/a.dll"
        );
        let msg = format!(
            "the ONNX Runtime library is missing: not at {} nor at {}; data at {}",
            in_models.display(),
            in_engine.display(),
            root.join("diarist.db").display()
        );
        let scrubbed = scrub_message(&paths, &msg);
        assert_eq!(
            scrubbed,
            format!(
                "the ONNX Runtime library is missing: not at <models>{s}p{s}diarist-onnxruntime.dll \
                 nor at <engine>{s}onnxruntime-1.31.0{s}diarist-onnxruntime.dll; data at <data \
                 root>{s}diarist.db",
                s = std::path::MAIN_SEPARATOR
            )
        );
        // A runtime failure from current(), end to end: the stored message
        // names no folder of the temp root absolutely.
        let paths = DataPaths::at(&root);
        let store = crate::store::MemoryStore::new();
        let m = paths.models().join("punct");
        std::fs::create_dir_all(&m).unwrap();
        std::fs::write(m.join("model.onnx"), b"x").unwrap();
        std::fs::write(m.join("tokenizer.json"), b"{}").unwrap();
        std::fs::write(
            m.join("model.onnx.meet.json"),
            r#"{"punctuation": {"tokenizer": "tokenizer.json", "max_tokens": 64,
                "heads": [{"output": "logits", "kind": "post", "labels": ["", "."],
                           "position": "last_token", "values": "logits"}]}}"#,
        )
        .unwrap();
        crate::models::scan(&paths, &store, &Manifest::builtin()).unwrap();
        let e = match current(&paths, &store) {
            Err(e) => e,
            Ok(_) => panic!("no library: current() must fail"),
        };
        assert_eq!(e.reason, PunctReason::Runtime);
        assert!(
            !e.message.contains(&dir.path().display().to_string())
                && e.message.contains("<models>")
                && e.message.contains("<engine>"),
            "{}",
            e.message
        );
    }

    /// The manifest's hash of the library is used only for the model
    /// folder's own library of a manifest model.
    #[test]
    fn the_manifest_hash_is_for_the_folders_own_library() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("m");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(RUNTIME_DLL), b"MZ").unwrap();
        let shared = dir.path().join("engine").join(RUNTIME_DLL);
        std::fs::create_dir_all(shared.parent().unwrap()).unwrap();
        std::fs::write(&shared, b"MZ").unwrap();
        let spec = PunctSpec::from_json(
            r#"{"tokenizer": "tokenizer.json", "max_tokens": 64,
                "heads": [{"output": "logits", "kind": "post", "labels": ["", "."],
                           "position": "last_token", "values": "logits"}]}"#,
        )
        .unwrap();
        let entry: ModelEntry = serde_json::from_value(serde_json::json!({
            "name": "p", "role": "punctuation", "file": "model.onnx", "url": "", "size_bytes": 3,
            "sha256": "1".repeat(64),
            "files": [
                {"path": "tokenizer.json", "url": "", "size_bytes": 1, "sha256": "2".repeat(64)},
                {"path": "./Diarist-OnnxRuntime.dll", "url": "", "size_bytes": 1, "sha256": "3".repeat(64)}
            ]
        }))
        .unwrap();
        let own = root.join(RUNTIME_DLL);
        assert_eq!(
            manifest_runtime_sha(Some(&entry), &spec, &root, &own),
            Some("3".repeat(64))
        );
        assert_eq!(
            manifest_runtime_sha(None, &spec, &root, &own),
            None,
            "a drop-in"
        );
        assert_eq!(
            manifest_runtime_sha(Some(&entry), &spec, &root, &shared),
            None,
            "a shared copy"
        );
        let mut unlisted = entry.clone();
        unlisted.files.pop();
        assert_eq!(
            manifest_runtime_sha(Some(&unlisted), &spec, &root, &own),
            None
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
                &[0, 0],
                &[tok(&[true, false, false]), tok(&[false, false])]
            ),
            Case::First
        );
        assert_eq!(
            char_case(
                "nasa",
                &p(&["na", "sa"]),
                &[0, 0],
                &[tok(&[true, true]), tok(&[true, true])]
            ),
            Case::Upper
        );
        assert_eq!(
            char_case(
                "iphone",
                &p(&["iph", "one"]),
                &[0, 0],
                &[tok(&[false, true, false]), tok(&[false; 3])]
            ),
            Case::Chars(vec![false, true, false, false, false, false])
        );
        // The pieces do not spell the word (normalisation): first letter only.
        assert_eq!(
            char_case(
                "ﬁne",
                &p(&["fine"]),
                &[0],
                &[tok(&[true, false, false, false])]
            ),
            Case::First
        );
        assert_eq!(
            char_case("the", &p(&["the"]), &[0], &[tok(&[false; 3])]),
            Case::Keep
        );
        // A second piece without a marker after one with it.
        assert_eq!(
            char_case(
                "mcdonald",
                &p(&["mc", "donald"]),
                &[1, 0],
                &[tok(&[true, true, false]), tok(&[true, false, false])]
            ),
            Case::Chars(vec![true, false, true, false, false, false, false, false])
        );
    }

    /// C's description block as the spike wrote it (`punct-C`), its heads
    /// only.
    const C_HEADS: &str = r#"{
        "tokenizer": "tokenizer.json", "input_casing": "lower", "max_tokens": 256,
        "bos_id": 1, "eos_id": 2, "pad_id": 3, "inputs": {"ids": "input_ids"},
        "heads": [
            {"output": "pre_preds", "kind": "pre", "labels": ["<NULL>", "¿"], "position": "first_token", "values": "decisions"},
            {"output": "post_preds", "kind": "post", "labels": ["<NULL>", "<ACRONYM>", ".", ",", "?"], "position": "per_subword", "values": "decisions"},
            {"output": "cap_preds", "kind": "case", "position": "per_subword", "values": "decisions"},
            {"output": "seg_preds", "kind": "boundary", "position": "per_subword", "values": "decisions"}
        ],
        "casing": "per_char"
    }"#;

    /// C's real `cap_preds` layout (the reviewer's `c_caps.py` on
    /// `punct_cap_seg_en.onnx`, 16 flags per token): the `▁` of a piece is
    /// flag 0, so "monday" is `[0, 1, 0, 0, 0, 0, 0]`. Read one place too
    /// early, that gave "mOnday" and "JOhn".
    #[test]
    fn per_character_casing_counts_the_word_marker() {
        let words = ["on", "monday", "in", "paris", "with", "john", "mcdonald"];
        let flags: [&[u8]; 7] = [
            &[0, 0, 0],
            &[0, 1, 0, 0, 0, 0, 0],
            &[0, 0, 0],
            &[1, 1, 0, 0, 0, 0],
            &[0, 0, 0, 0, 0],
            &[1, 1, 0, 0, 0],
            &[1, 1, 0, 1, 0, 0, 0, 0, 0],
        ];
        let mut enc = Encoded::default();
        let mut toks = Vec::new();
        for (w, (word, f)) in words.iter().zip(flags).enumerate() {
            let piece = format!("\u{2581}{word}");
            let (lead, text) = super::super::tokenize::strip_marker(&piece);
            enc.ids.push(w as i64);
            enc.word_of.push(w);
            enc.ends_word.push(true);
            enc.pieces.push(text.to_owned());
            enc.lead.push(lead);
            let mut f: Vec<bool> = f.iter().map(|&x| x != 0).collect();
            f.resize(16, false);
            toks.push(tok(&f));
        }
        let heads = Heads::of(&PunctSpec::from_json(C_HEADS).unwrap());
        let model_words: Vec<String> = words.iter().map(|w| (*w).to_owned()).collect();
        let labels = heads.word_labels(&mut LabelStats::default(), &model_words, &enc, &toks);
        assert_eq!(
            labels.iter().map(|l| l.case.clone()).collect::<Vec<_>>(),
            [
                Case::Keep,
                Case::First,
                Case::Keep,
                Case::First,
                Case::Keep,
                Case::First,
                Case::Chars(vec![true, false, true, false, false, false, false, false]),
            ]
        );
        // Through Diarist's label application (apply.rs).
        let source: Vec<crate::transcript::TranscriptWord> = words
            .iter()
            .enumerate()
            .map(|(i, w)| crate::transcript::TranscriptWord {
                channel: crate::types::Channel::Source,
                start_ms: i as i64 * 400,
                end_ms: i as i64 * 400 + 300,
                text: (*w).to_owned(),
                confidence: None,
                speaker: "spk_0".into(),
                raw_label: Some("spk_0".into()),
                raw_text: None,
                margin: None,
                overlap: false,
            })
            .collect();
        let mut channels = vec![(crate::types::Channel::Source, source)];
        let given = labels.clone();
        let mut p = super::super::fixture::FixturePunctuator::new(move |_, _| Ok(given.clone()));
        let report = super::super::punctuate_channels(
            &mut channels,
            Some(&mut p as &mut dyn Punctuator),
            &PunctOptions::default(),
            &super::super::RunLanguage::new("en"),
            &|| false,
        );
        assert_eq!(report.rejected_words, Some(0), "{report:?}");
        let shown: Vec<&str> = channels[0].1.iter().map(|w| w.text.as_str()).collect();
        // "On" is the sentence-start rule; the rest is the model's casing.
        assert_eq!(
            shown,
            ["On", "Monday", "in", "Paris", "with", "John", "McDonald"]
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
        let has_case = spec.head(HeadKind::Case).is_some();
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
            None,
            4,
        )
        .unwrap();
        let load_s = t0.elapsed().as_secs_f64();
        assert!(
            !Path::new(&p.descriptor().runtime.path).is_absolute(),
            "{}",
            p.descriptor().runtime.path
        );
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
        // Without a casing head (A) nothing changes letters, and each marked
        // word keeps its normalised text (plan 16 section 3.3 rule 3).
        let marks = a.iter().filter(|l| l.post.is_some()).count();
        let cased = a.iter().filter(|l| l.case != Case::Keep).count();
        assert!(has_case || cased == 0);
        assert!(a.iter().all(|l| l.post.is_none_or(|m| !m.is_pre())));
        assert!(a.iter().all(|l| l.pre.is_none_or(|m| m.is_pre())));
        eprintln!("{marks} marks, {cased} casing labels");
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
        // Through Diarist's label application (chunk A's apply.rs), which
        // checks every invariant of plan 16 section 3.3 per word: nothing is
        // rejected or refused, and only the display text changes.
        let source: Vec<crate::transcript::TranscriptWord> = words
            .iter()
            .enumerate()
            .map(|(i, w)| crate::transcript::TranscriptWord {
                channel: crate::types::Channel::Source,
                start_ms: i as i64 * 400,
                end_ms: i as i64 * 400 + 300,
                text: (*w).to_owned(),
                confidence: None,
                speaker: "spk_0".into(),
                raw_label: Some("spk_0".into()),
                raw_text: None,
                margin: None,
                overlap: false,
            })
            .collect();
        let mut channels = vec![(crate::types::Channel::Source, source.clone())];
        let opts = p.options();
        let report = super::super::punctuate_channels(
            &mut channels,
            Some(&mut p as &mut dyn Punctuator),
            &opts,
            &super::super::RunLanguage::new("en"),
            &|| false,
        );
        assert_eq!(report.reason, PunctReason::Ok, "{report:?}");
        assert_eq!(report.rejected_words, Some(0), "{report:?}");
        assert_eq!(report.streams_refused.unwrap_or(0), 0, "{report:?}");
        let out = &channels[0].1;
        assert_eq!(out.len(), source.len());
        for (o, s) in out.iter().zip(&source) {
            assert_eq!(
                (o.start_ms, o.end_ms, &o.speaker),
                (s.start_ms, s.end_ms, &s.speaker)
            );
            assert_eq!(norm(&o.text), norm(&s.text), "{} -> {}", s.text, o.text);
            assert_eq!(o.raw_text.is_some(), o.text != s.text);
        }
        eprintln!(
            "applied: {} marks, {} capitals",
            report.marks_added.unwrap_or(0),
            report.case_upgrades.unwrap_or(0)
        );

        // A model with a casing head (B, C) capitalises names inside a
        // sentence, letter by letter (the `▁` offset of its flags).
        if has_case {
            let sentence = "hello i flew to paris with nasa engineers on monday and we met \
                            john mcdonald at the united nations";
            let words: Vec<&str> = sentence.split_whitespace().collect();
            let mut channels = vec![(
                crate::types::Channel::Source,
                words
                    .iter()
                    .enumerate()
                    .map(|(i, w)| crate::transcript::TranscriptWord {
                        channel: crate::types::Channel::Source,
                        start_ms: i as i64 * 400,
                        end_ms: i as i64 * 400 + 300,
                        text: (*w).to_owned(),
                        confidence: None,
                        speaker: "spk_0".into(),
                        raw_label: Some("spk_0".into()),
                        raw_text: None,
                        margin: None,
                        overlap: false,
                    })
                    .collect::<Vec<_>>(),
            )];
            let report = super::super::punctuate_channels(
                &mut channels,
                Some(&mut p as &mut dyn Punctuator),
                &opts,
                &super::super::RunLanguage::new("en"),
                &|| false,
            );
            assert_eq!(report.rejected_words, Some(0), "{report:?}");
            let shown: Vec<String> = channels[0]
                .1
                .iter()
                .map(|w| {
                    w.text
                        .trim_end_matches(|c: char| !c.is_alphanumeric())
                        .to_owned()
                })
                .collect();
            eprintln!("cased: {}", shown.join(" "));
            for (at, want) in [(4, "Paris"), (9, "Monday"), (13, "John"), (14, "McDonald")] {
                assert_eq!(shown[at], want, "{shown:?}");
            }
        }

        // Cancel is honoured.
        assert!(matches!(
            p.label_stream(&long, &hint, &|| true),
            Err(CoreError::Cancelled)
        ));

        // The identity check's failure path names both files, and another
        // release cannot be loaded beside ours.
        let other = PathBuf::from(r"C:\Windows\System32\onnxruntime.dll");
        let e = identity_check(&rt.path, &other).unwrap_err();
        assert_eq!(e.reason, PunctReason::Runtime);
        assert!(e.message.contains("System32") && e.message.contains(RUNTIME_DLL_NAME));
        if other.is_file() {
            let want = RuntimeWant {
                version: Some("1.17".into()),
                sha256: None,
            };
            let e = ensure_runtime(&other, &want).unwrap_err();
            assert_eq!(e.reason, PunctReason::Runtime);
            assert!(e.message.contains("restart"), "{}", e.message);
        }
        // A second model folder with its own copy of the same release is
        // served by the loaded library, without a restart.
        let second = tempfile::tempdir().unwrap();
        let copy = second.path().join(RUNTIME_DLL_NAME);
        std::fs::copy(&rt.path, &copy).unwrap();
        let want = RuntimeWant {
            version: Some(rt.version.clone()),
            sha256: None,
        };
        let (again, _) = ensure_runtime(&copy, &want).unwrap();
        assert_eq!(again, rt, "the loaded module serves it");

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

    /// The runtime from a model folder whose library path is past
    /// `MAX_PATH` (chunk D found 251 characters failing): a junction to
    /// `DIARIST_PUNCT_MODEL` under a 300-character folder. The library
    /// loads, the identity check passes, the recorded path is the plain
    /// form (no `\\?\`), and the model labels. ort's environment is
    /// process-wide, so run it alone, in its own process:
    /// `cargo test -p diarist-core --lib punct::onnx::tests::real_model_from_a_long_path -- --ignored --exact`.
    #[test]
    #[ignore = "needs DIARIST_PUNCT_MODEL; run alone"]
    #[cfg(windows)]
    fn real_model_from_a_long_path() {
        let Some(folder) = std::env::var_os("DIARIST_PUNCT_MODEL").map(PathBuf::from) else {
            eprintln!("skipped: DIARIST_PUNCT_MODEL is not set");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let long = crate::engine::process::long_test_dir(tmp.path(), 300);
        let link = long.join("m");
        let made = std::process::Command::new("cmd")
            .arg("/c")
            .arg("mklink")
            .arg("/J")
            .arg(crate::engine::process::path_arg(&link))
            .arg(&folder)
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");
        let result = std::panic::catch_unwind(|| {
            let model = link.join("onnx").join("model_quantized.onnx");
            let dll = link.join(RUNTIME_DLL_NAME);
            assert!(dll.as_os_str().len() > 300, "{}", dll.display());
            let sidecar = crate::models::Sidecar::read(&model).unwrap().unwrap();
            let spec = sidecar.punctuation.expect("a punctuation block");
            let paths = DataPaths::at(tmp.path()).with_models_dir(&link);
            let mut p = OnnxPunctuator::open(
                &model,
                spec,
                PunctModelInfo {
                    name: "test".into(),
                    languages: vec!["*".into()],
                    ..PunctModelInfo::default()
                },
                &paths,
                None,
                4,
            )
            .unwrap_or_else(|e| panic!("{}: {}", dll.display(), e.message));
            let rt = loaded_runtime().unwrap();
            assert!(!rt.path.to_string_lossy().starts_with(r"\\?\"), "{rt:?}");
            assert!(same_file(&rt.path, &dll), "{rt:?}");
            assert!(rt.version.starts_with("1.31"), "{}", rt.version);
            let words: Vec<&str> = FIXTURE.split_whitespace().collect();
            let hint = LangHint {
                language: Some("en".into()),
                script: Script::Latin,
            };
            let labels = p.label_stream(&words, &hint, &|| false).unwrap();
            assert_eq!(labels.len(), words.len());
            assert!(labels.iter().any(|l| l.post.is_some()), "{labels:?}");
            // The module's own name is the extended form; the check
            // compares the files, not the spelling.
            identity_check(&rt.path, &PathBuf::from(format!(r"\\?\{}", dll.display()))).unwrap();
        });
        // Remove the junction itself, never what it points to.
        std::fs::remove_dir(&link).unwrap();
        assert!(folder.join(RUNTIME_DLL_NAME).is_file());
        if let Err(e) = result {
            std::panic::resume_unwind(e);
        }
    }

    /// Chunk B's CLI check, step 1 (plan 16 section 7.2 B): a scratch data
    /// root (`DIARIST_PUNCT_CHECK_ROOT`) with one meeting imported through
    /// the file import from a cached `transcribe --json` output
    /// (`DIARIST_PUNCT_CHECK_WORDS`, default the 5-minute AMI fixture), no
    /// engine and no GPU. Then `diarist-cli --data-dir <root> --models-dir
    /// <model folder> punctuate <id>` and the exports before and after.
    #[test]
    #[ignore = "needs DIARIST_PUNCT_CHECK_ROOT (an empty scratch folder)"]
    fn cli_check_root() {
        use crate::audio::{DecodedInfo, Decoder, DecoderKind};
        use crate::engine::nemo_cli::{engine_words, file_events, parse_transcribe_json};
        use crate::engine::{Engine, EngineEvent, EngineInfo, EngineWord, FileJob};
        use crate::pipeline::{import_file, CancelToken, ImportOptions, ImportRequest};
        use crate::store::{SqliteStore, Store};
        use crate::types::JobId;
        use crate::views::Settings;

        struct Replay {
            words: Vec<EngineWord>,
            audio_us: u64,
        }
        impl Engine for Replay {
            fn transcribe_file(
                &self,
                job: FileJob,
            ) -> crate::Result<std::sync::mpsc::Receiver<EngineEvent>> {
                let (tx, rx) = std::sync::mpsc::channel();
                for ev in file_events(
                    &job.job_id,
                    crate::types::Channel::Source,
                    self.words.clone(),
                    self.audio_us,
                    std::time::Duration::from_secs(1),
                ) {
                    tx.send(ev).unwrap();
                }
                Ok(rx)
            }
            fn cancel(&self, _: JobId) {}
            fn info(&self) -> EngineInfo {
                EngineInfo {
                    runtime: "replay of a cached transcribe --json".into(),
                    ..Default::default()
                }
            }
        }
        struct Silence(i64);
        impl Decoder for Silence {
            fn decode_to_wav16k(&self, _: &Path, output: &Path) -> crate::Result<DecodedInfo> {
                let spec = hound::WavSpec {
                    channels: 1,
                    sample_rate: 16_000,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                };
                let mut w = hound::WavWriter::create(output, spec).unwrap();
                for _ in 0..16_000 {
                    w.write_sample(0i16).unwrap();
                }
                w.finalize().unwrap();
                Ok(DecodedInfo {
                    duration_ms: self.0,
                    source_sample_rate: 16_000,
                    source_channels: 1,
                    decoder: DecoderKind::Other,
                })
            }
        }

        let Some(root) = std::env::var_os("DIARIST_PUNCT_CHECK_ROOT").map(PathBuf::from) else {
            eprintln!("skipped: DIARIST_PUNCT_CHECK_ROOT is not set");
            return;
        };
        let words_json = std::env::var_os("DIARIST_PUNCT_CHECK_WORDS")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/nemotron35_transcribe_diarize_5min.json")
            });
        let out = parse_transcribe_json(&std::fs::read_to_string(&words_json).unwrap()).unwrap();
        let words = engine_words(&out);
        let end_us = words.iter().map(|w| w.t1_us).max().unwrap_or(0);
        let audio_us = ((out.duration * 1e6) as u64).max(end_us);
        std::fs::create_dir_all(&root).unwrap();
        let store = SqliteStore::open(root.join("diarist.db")).unwrap();
        let src = root.join("replayed.wav");
        std::fs::write(&src, b"replayed: the engine is a cached words file").unwrap();
        let opts = ImportOptions {
            min_free_bytes: 0,
            ..ImportOptions::new(&DataPaths::at(&root), &Settings::default())
        };
        let id = import_file(
            &store,
            &Replay {
                words: words.clone(),
                audio_us,
            },
            &Silence((audio_us / 1_000) as i64),
            ImportRequest {
                job_id: JobId("punct-cli-check".into()),
                path: src,
                title: Some(
                    words_json
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                ),
                diarize: true,
            },
            &opts,
            &CancelToken::default(),
            &|_| {},
        )
        .unwrap();
        let stored = store.get_words(id, i64::MIN, i64::MAX).unwrap().len();
        println!(
            "meeting {} ({} words from {}, {:.1} min of audio)",
            id.0,
            stored,
            words_json.display(),
            audio_us as f64 / 60e6
        );
    }
}
