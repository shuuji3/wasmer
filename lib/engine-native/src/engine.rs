//! Native Engine.

use crate::NativeArtifact;
use libloading::Library;
use loupe::MemoryUsage;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use wasmer_compiler::{CompileError, Target};
#[cfg(feature = "compiler")]
use wasmer_compiler::{Compiler, Triple};
use wasmer_engine::{Artifact, DeserializeError, Engine, EngineId, Tunables};
#[cfg(feature = "compiler")]
use wasmer_types::Features;
use wasmer_types::FunctionType;
use wasmer_vm::{SignatureRegistry, VMSharedSignatureIndex};

/// A WebAssembly `Native` Engine.
#[derive(Clone, MemoryUsage)]
pub struct NativeEngine {
    inner: Arc<Mutex<NativeEngineInner>>,
    /// The target for the compiler
    target: Arc<Target>,
    engine_id: EngineId,
}

impl NativeEngine {
    /// Create a new `NativeEngine` with the given config
    #[cfg(feature = "compiler")]
    pub fn new(compiler: Box<dyn Compiler>, target: Target, features: Features) -> Self {
        let is_cross_compiling = *target.triple() != Triple::host();
        let linker = Linker::find_linker(is_cross_compiling);

        Self {
            inner: Arc::new(Mutex::new(NativeEngineInner {
                compiler: Some(compiler),
                signatures: SignatureRegistry::new(),
                prefixer: None,
                features,
                is_cross_compiling,
                linker,
                libraries: vec![],
            })),
            target: Arc::new(target),
            engine_id: EngineId::default(),
        }
    }

    /// Create a headless `NativeEngine`
    ///
    /// A headless engine is an engine without any compiler attached.
    /// This is useful for assuring a minimal runtime for running
    /// WebAssembly modules.
    ///
    /// For example, for running in IoT devices where compilers are very
    /// expensive, or also to optimize startup speed.
    ///
    /// # Important
    ///
    /// Headless engines can't compile or validate any modules,
    /// they just take already processed Modules (via `Module::serialize`).
    pub fn headless() -> Self {
        Self {
            inner: Arc::new(Mutex::new(NativeEngineInner {
                #[cfg(feature = "compiler")]
                compiler: None,
                #[cfg(feature = "compiler")]
                features: Features::default(),
                signatures: SignatureRegistry::new(),
                prefixer: None,
                is_cross_compiling: false,
                linker: Linker::None,
                libraries: vec![],
            })),
            target: Arc::new(Target::default()),
            engine_id: EngineId::default(),
        }
    }

    /// Sets a prefixer for the wasm module, so we can avoid any collisions
    /// in the exported function names on the generated shared object.
    ///
    /// This, allows us to rather than have functions named `wasmer_function_1`
    /// to be named `wasmer_function_PREFIX_1`.
    ///
    /// # Important
    ///
    /// This prefixer function should be deterministic, so the compilation
    /// remains deterministic.
    pub fn set_deterministic_prefixer<F>(&mut self, prefixer: F)
    where
        F: Fn(&[u8]) -> String + Send + 'static,
    {
        let mut inner = self.inner_mut();
        inner.prefixer = Some(Box::new(prefixer));
    }

    pub(crate) fn inner(&self) -> std::sync::MutexGuard<'_, NativeEngineInner> {
        self.inner.lock().unwrap()
    }

    pub(crate) fn inner_mut(&self) -> std::sync::MutexGuard<'_, NativeEngineInner> {
        self.inner.lock().unwrap()
    }
}

impl Engine for NativeEngine {
    /// The target
    fn target(&self) -> &Target {
        &self.target
    }

    /// Register a signature
    fn register_signature(&self, func_type: &FunctionType) -> VMSharedSignatureIndex {
        let compiler = self.inner();
        compiler.signatures().register(func_type)
    }

    /// Lookup a signature
    fn lookup_signature(&self, sig: VMSharedSignatureIndex) -> Option<FunctionType> {
        let compiler = self.inner();
        compiler.signatures().lookup(sig)
    }

    /// Validates a WebAssembly module
    fn validate(&self, binary: &[u8]) -> Result<(), CompileError> {
        self.inner().validate(binary)
    }

    /// Compile a WebAssembly binary
    #[cfg(feature = "compiler")]
    fn compile(
        &self,
        binary: &[u8],
        tunables: &dyn Tunables,
    ) -> Result<Arc<dyn Artifact>, CompileError> {
        Ok(Arc::new(NativeArtifact::new(&self, binary, tunables)?))
    }

    /// Compile a WebAssembly binary (it will fail because the `compiler` flag is disabled).
    #[cfg(not(feature = "compiler"))]
    fn compile(
        &self,
        _binary: &[u8],
        _tunables: &dyn Tunables,
    ) -> Result<Arc<dyn Artifact>, CompileError> {
        Err(CompileError::Codegen(
            "The `NativeEngine` is operating in headless mode, so it cannot compile a module."
                .to_string(),
        ))
    }

    /// Deserializes a WebAssembly module (binary content of a Shared Object file)
    unsafe fn deserialize(&self, bytes: &[u8]) -> Result<Arc<dyn Artifact>, DeserializeError> {
        Ok(Arc::new(NativeArtifact::deserialize(&self, &bytes)?))
    }

    /// Deserializes a WebAssembly module from a path
    /// It should point to a Shared Object file generated by this engine.
    unsafe fn deserialize_from_file(
        &self,
        file_ref: &Path,
    ) -> Result<Arc<dyn Artifact>, DeserializeError> {
        Ok(Arc::new(NativeArtifact::deserialize_from_file(
            &self, &file_ref,
        )?))
    }

    fn id(&self) -> &EngineId {
        &self.engine_id
    }

    fn cloned(&self) -> Arc<dyn Engine + Send + Sync> {
        Arc::new(self.clone())
    }
}

#[derive(Clone, Copy, MemoryUsage)]
pub(crate) enum Linker {
    None,
    Clang11,
    Clang10,
    Clang,
    Gcc,
}

impl Linker {
    #[cfg(feature = "compiler")]
    fn find_linker(is_cross_compiling: bool) -> Self {
        let (possibilities, requirements): (&[_], _) = if is_cross_compiling {
            (
                &[Linker::Clang11, Linker::Clang10, Linker::Clang],
                "at least one of `clang-11`, `clang-10`, or `clang`",
            )
        } else {
            (&[Linker::Gcc], "`gcc`")
        };
        *possibilities
            .iter()
            .filter(|linker| which::which(linker.executable()).is_ok())
            .next()
            .unwrap_or_else(|| {
                panic!(
                    "Need {} installed in order to use `NativeEngine` when {}cross-compiling",
                    requirements,
                    if is_cross_compiling { "" } else { "not " }
                )
            })
    }

    pub(crate) fn executable(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Clang11 => "clang-11",
            Self::Clang10 => "clang-10",
            Self::Clang => "clang",
            Self::Gcc => "gcc",
        }
    }
}

/// The inner contents of `NativeEngine`
#[derive(MemoryUsage)]
pub struct NativeEngineInner {
    /// The compiler
    #[cfg(feature = "compiler")]
    compiler: Option<Box<dyn Compiler>>,

    /// The WebAssembly features to use
    #[cfg(feature = "compiler")]
    features: Features,

    /// The signature registry is used mainly to operate with trampolines
    /// performantly.
    signatures: SignatureRegistry,

    /// The prefixer returns the a String to prefix each of
    /// the functions in the shared object generated by the `NativeEngine`,
    /// so we can assure no collisions.
    #[loupe(skip)]
    prefixer: Option<Box<dyn Fn(&[u8]) -> String + Send>>,

    /// Whether the native engine will cross-compile.
    is_cross_compiling: bool,

    /// The linker to use.
    linker: Linker,

    /// List of libraries loaded by this engine.
    #[loupe(skip)]
    libraries: Vec<Library>,
}

impl NativeEngineInner {
    /// Gets the compiler associated to this engine.
    #[cfg(feature = "compiler")]
    pub fn compiler(&self) -> Result<&dyn Compiler, CompileError> {
        if self.compiler.is_none() {
            return Err(CompileError::Codegen("The `NativeEngine` is operating in headless mode, so it can only execute already compiled Modules.".to_string()));
        }
        Ok(&**self
            .compiler
            .as_ref()
            .expect("Can't get compiler reference"))
    }

    #[cfg(feature = "compiler")]
    pub(crate) fn get_prefix(&self, bytes: &[u8]) -> String {
        if let Some(prefixer) = &self.prefixer {
            prefixer(&bytes)
        } else {
            "".to_string()
        }
    }

    #[cfg(feature = "compiler")]
    pub(crate) fn features(&self) -> &Features {
        &self.features
    }

    /// Validate the module
    #[cfg(feature = "compiler")]
    pub fn validate<'data>(&self, data: &'data [u8]) -> Result<(), CompileError> {
        self.compiler()?.validate_module(self.features(), data)
    }

    /// Validate the module
    #[cfg(not(feature = "compiler"))]
    pub fn validate<'data>(&self, _data: &'data [u8]) -> Result<(), CompileError> {
        Err(CompileError::Validate(
            "The `NativeEngine` is not compiled with compiler support, which is required for validating".to_string(),
        ))
    }

    /// Shared signature registry.
    pub fn signatures(&self) -> &SignatureRegistry {
        &self.signatures
    }

    pub(crate) fn is_cross_compiling(&self) -> bool {
        self.is_cross_compiling
    }

    pub(crate) fn linker(&self) -> Linker {
        self.linker
    }

    pub(crate) fn add_library(&mut self, library: Library) {
        self.libraries.push(library);
    }
}
