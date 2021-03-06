//! The module system.

mod metadata;
#[cfg(test)]
mod tests;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write;
use std::path::{Path, PathBuf};

use failure::ResultExt;
use symbol::Symbol;

use anf::Module;
use ast::Attr;
use error::{Error, ErrorKind};
use flatanf::Program;
pub use modules::metadata::{
    BinaryComponentMetadata, ComponentsMetadata, DependencyMetadata, LibraryComponentMetadata,
    PackageMetadata,
};
use parser::parse_file;
use BuiltinPackage;

#[derive(Clone, Debug)]
enum Package {
    Builtins(HashMap<Symbol, HashSet<Symbol>>),
    Filesystem(PathBuf, PackageMetadata, Vec<Module>),
}

/// The module context.
///
/// The `Left` alternative represents a builtin module, while the `Right`
/// alternative is a loaded module.
#[derive(Clone, Debug)]
pub struct Packages {
    pkgs: HashMap<Symbol, Package>,
    std_name: Option<Symbol>,
}

impl Packages {
    /// Creates a new `Packages` instance.
    pub fn new() -> Packages {
        Packages {
            pkgs: HashMap::new(),
            std_name: None,
        }
    }

    /// Adds a builtin package.
    pub fn add_builtins<P: BuiltinPackage>(&mut self) {
        self.pkgs.insert(P::name(), Package::Builtins(P::decls()));
    }

    /// Loads the modules in the package in the given directory, returning the
    /// package's name.
    ///
    /// This includes both the modules declared by the package in the directory
    /// and any dependencies.
    pub fn add_modules_from(&mut self, path: PathBuf) -> Result<Symbol, Error> {
        let root_meta = self.load_metadata_from(&path)?;
        for (dep_name, dep_meta) in &root_meta.dependencies {
            warn!("TODO Load {} {:#?}", dep_name, dep_meta);
            // TODO Load dependencies.
        }
        let main_files = root_meta
            .components
            .binaries
            .iter()
            .map(|c| &c.path as &str)
            .filter(|p| p.starts_with("src/"))
            .map(|p| &p[4..])
            .collect::<Vec<_>>();
        self.add_package_from(root_meta.name, path, false, &main_files)?;
        Ok(root_meta.name)
    }

    /// Loads a package from the given directory, without loading dependencies.
    /// However, this will panic if the dependencies are not already met.
    pub fn add_package_from(
        &mut self,
        package_name: Symbol,
        path: PathBuf,
        require_library: bool,
        main_files: &[&str],
    ) -> Result<(), Error> {
        let meta = self.load_metadata_from(&path)?;
        if package_name != meta.name {
            return Err(ErrorKind::MisnamedPackage(package_name, meta.name).into());
        } else if meta.components.library.is_none() {
            if require_library {
                return Err(ErrorKind::DependencyMustExportLib(package_name).into());
            } else {
                self.pkgs
                    .insert(package_name, Package::Filesystem(path, meta, Vec::new()));
                return Ok(());
            };
        }

        let mut modules = Vec::new();
        let src_path = path.join("src");
        let lib_oft_path = src_path.join("lib.oft");

        fn crawl(
            package_name: Symbol,
            modules: &mut Vec<Module>,
            mod_stack: &mut Vec<Symbol>,
            base: PathBuf,
            lib_oft_path: &Path,
            main_files: &[&str],
        ) -> Result<(), Error> {
            // TODO: This could use a good catch block...
            for entry in base.read_dir().with_context(|_| {
                ErrorKind::CouldntReadPackageDir(base.display().to_string(), package_name)
            })? {
                let entry = entry.with_context(|_| {
                    ErrorKind::CouldntReadPackageDir(base.display().to_string(), package_name)
                })?;
                let file_type = entry.file_type().with_context(|_| {
                    ErrorKind::CouldntReadPackageDir(base.display().to_string(), package_name)
                })?;
                if file_type.is_dir() {
                    let name = entry
                        .file_name()
                        .into_string()
                        .expect("Non-Unicode source directory name...")
                        .into();
                    mod_stack.push(name);
                    crawl(
                        package_name,
                        modules,
                        mod_stack,
                        entry.path(),
                        lib_oft_path,
                        main_files,
                    )?;
                    assert_eq!(mod_stack.pop(), Some(name));
                } else if file_type.is_file() {
                    let file_name: PathBuf = entry.file_name().into();
                    if main_files.iter().any(|p| {
                        let p: &Path = p.as_ref();
                        p == file_name
                    }) {
                        debug!("Skipping src/{}...", file_name.display());
                        continue;
                    }
                    if file_name.extension() != Some("oft".as_ref()) {
                        continue;
                    }
                    let file_name = file_name.file_stem().unwrap();

                    let mut name = String::new();
                    for &mod_part in mod_stack.iter() {
                        write!(name, "{}/", mod_part).unwrap();
                    }
                    let path = entry.path();
                    if path == lib_oft_path {
                        assert_eq!(name.pop(), Some('/'));
                    } else {
                        name += file_name.to_str().expect("Non-Unicode source file name...");
                    }
                    let name = name.into();

                    let module = Packages::load_module(&path)?;
                    if name != module.name {
                        return Err(ErrorKind::MisnamedModule(name, module.name).into());
                    }

                    modules.push(module);
                } else {
                    warn!(
                        "Source file `{}' is neither directory nor file",
                        entry.path().display()
                    );
                }
            }
            Ok(())
        }
        crawl(
            package_name,
            &mut modules,
            &mut vec![package_name],
            src_path,
            &lib_oft_path,
            main_files,
        )?;

        self.pkgs.insert(
            package_name,
            Package::Filesystem(path, meta, modules.into_iter().collect()),
        );

        Ok(())
    }

    /// Loads the stdlib from the given directory. Will panic if any
    /// dependencies are requested.
    pub fn add_stdlib_from(&mut self, path: PathBuf) -> Result<(), Error> {
        // TODO: This is a DRY violation with add_modules_from.
        let root_meta = self.load_metadata_from(&path)?;
        assert!(root_meta.dependencies.is_empty());
        self.std_name = Some(root_meta.name);
        let main_files = root_meta
            .components
            .binaries
            .iter()
            .map(|c| &c.path as &str)
            .filter(|p| p.starts_with("src/"))
            .map(|p| &p[4..])
            .collect::<Vec<_>>();
        self.add_package_from(root_meta.name, path, true, &main_files)
    }

    /// Compiles a binary from a given module into a `flatanf::Program`.
    pub fn compile(self, root_package_name: Symbol, binary: &str) -> Result<Program, Error> {
        let mut builtins = HashMap::new();
        let mut root_meta_path = None;
        let mut mods = Vec::new();

        // Extract the prelude module's exports.
        let prelude = format!("{}/prelude", self.std_name()).into();
        let prelude_exports = self.exports_of(self.std_name(), prelude)?
            .into_iter()
            .map(|n| (prelude, n))
            .collect::<Vec<_>>();
        let augment_module_imports = |m: &mut Module| {
            if !m.attrs.iter().any(|attr| *attr == Attr::NoPrelude)
                && !m.imports.iter().any(|&(m, _)| m == prelude)
            {
                m.imports.extend(prelude_exports.iter().cloned());
            }
        };

        // Bundle up the packages.
        for (package_name, package) in self.pkgs {
            match package {
                Package::Builtins(mods) => for (name, decls) in mods {
                    let name = if name.as_str() == "" {
                        package_name
                    } else {
                        format!("{}/{}", package_name, name).into()
                    };
                    builtins.insert(name, decls);
                },
                Package::Filesystem(path, meta, ms) => {
                    if root_package_name == package_name {
                        root_meta_path = Some((meta, path));
                    }
                    mods.extend(ms.into_iter().map(|mut m| {
                        augment_module_imports(&mut m);
                        m
                    }));
                }
            }
        }

        // Add the binary.
        let (root_meta, root_path) = match root_meta_path {
            Some((meta, path)) => (meta, path),
            None => {
                return Err(ErrorKind::NoSuchBinary(root_package_name, binary.to_string()).into());
            }
        };
        let binary_rel_path = root_meta
            .components
            .binaries
            .iter()
            .find(|bin| bin.name == binary)
            .map(|bin| &bin.path)
            .ok_or_else(|| ErrorKind::NoSuchBinary(root_package_name, binary.to_string()))?;
        let binary_path = root_path.join(binary_rel_path);
        let mut binary = Packages::load_module(binary_path)?;
        if binary.name != "main".into() {
            return Err(ErrorKind::BadBinaryName(binary.name).into());
        }
        augment_module_imports(&mut binary);
        mods.push(binary);

        // Create the `flatanf::Program`, run sanity checks, and return it.
        let program = Program::from_modules(mods, builtins)?;
        ::sanity::check(&program)?;
        Ok(program)
    }

    /// Returns the exports of the given module.
    pub fn exports_of(&self, package: Symbol, module: Symbol) -> Result<BTreeSet<Symbol>, Error> {
        match self.pkgs.get(&package) {
            Some(&Package::Builtins(ref package)) => {
                unimplemented!("{:?} {:?}", package, module);
            }
            Some(&Package::Filesystem(_, _, ref mods)) => {
                match mods.iter().find(|m| m.name == module) {
                    Some(m) => Ok(m.exports.clone()),
                    None => {
                        unimplemented!("{:?} {:?}", package, module);
                    }
                }
            }
            None => {
                unimplemented!("{:?} {:?}", package, module);
            }
        }
    }

    /// Loads metadata from the module in the given directory.
    pub fn load_metadata_from(&self, path: &Path) -> Result<PackageMetadata, Error> {
        let lits = parse_file(path.join("package.oftd"))?;
        PackageMetadata::from_literals(lits)
    }

    /// Loads an `anf::Module` from the given path.
    pub fn load_module<P: AsRef<Path>>(path: P) -> Result<Module, Error> {
        let lits = parse_file(path.as_ref())?;
        let ast_mod = ::ast::Module::from_values(path.as_ref(), lits)?;
        if let Some(name) = ast_mod
            .body
            .iter()
            .map(|decl| decl.name())
            .find(|name| name.contains(':'))
        {
            Err(ErrorKind::IllegalDeclName(name).into())
        } else {
            let anf_mod = Module::from(ast_mod);
            Ok(anf_mod)
        }
    }

    /// Returns the name of the standard library package, panicing if none has
    /// been loaded yet.
    pub fn std_name(&self) -> Symbol {
        self.std_name.expect("No std loaded yet!")
    }
}
