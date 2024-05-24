use std::{
    collections::BTreeSet,
    env, fs,
    io::{self, Write as _},
    path::Path,
};

use platforms::Platform;

fn from_build() -> Result<String, String> {
    let triple =
        env::var("RUSTUP_OVERRIDE_BUILD_TRIPLE").unwrap_or_else(|_| env::var("TARGET").unwrap());
    if Platform::ALL.iter().any(|p| p.target_triple == triple) {
        Ok(triple)
    } else {
        Err(triple)
    }
}

/// Generates the lists of known target architectures, OSes and environments.
fn generate_known_triples() -> io::Result<()> {
    /// Parses the given triple into 3 parts (target architecture, OS and environment).
    ///
    /// # Discussion
    ///
    /// The current model of target triples in Rustup requires some non-code knowledge to correctly generate the list.
    /// For example, the parsing results of two 2-dash triples can be different:
    ///
    /// ```jsonc
    /// { arch: aarch64, os: linux, env: android }
    /// { arch: aarch64, os: unknown-freebsd}
    /// ```
    ///
    /// Thus, the following parsing scheme is used:
    ///
    /// ```jsonc
    /// // for `x-y`
    /// { arch: x, os: y }
    ///
    /// // special case for `x-y-w` where `y` is `none` or `linux`
    /// // e.g. `thumbv4t-none-eabi`, `i686-linux-android`
    /// // (should've been called `x-unknown-y-w`, but alas)
    /// { arch: x, os: y, env: w }
    ///
    /// // for `x-y-z`
    /// { arch: x, os: y-z }
    ///
    /// // for `x-y-z-w`
    /// { arch: x, os: y-z, env: w }
    /// ```
    fn parse_triple(triple: &str) -> Option<(&str, &str, &str)> {
        match triple.split('-').collect::<Vec<_>>()[..] {
            [arch, os] => Some((arch, os, "")),
            [arch, os @ ("none" | "linux"), env] => Some((arch, os, env)),
            [arch, _, _] => Some((arch, &triple[(arch.len() + 1)..], "")),
            [arch, _, _, env] => Some((
                arch,
                &triple[(arch.len() + 1)..(triple.len() - env.len() - 1)],
                env,
            )),
            _ => None,
        }
    }

    let mut archs = BTreeSet::new();
    let mut oses = BTreeSet::new();
    let mut envs = BTreeSet::new();
    for (arch, os, env) in Platform::ALL
        .iter()
        .filter_map(|p| parse_triple(p.target_triple))
    {
        archs.insert(arch);
        oses.insert(os);
        if !env.is_empty() {
            envs.insert(env);
        }
    }

    let dst = Path::new(&env::var("OUT_DIR").unwrap()).join("known_triples.rs");
    let mut out_file = fs::File::create(dst)?;
    write!(
        out_file,
        r#"//
// This is genarated by `generate_known_triples()` in `build.rs`. Please do not modify.
//
"#,
    )?;

    writeln!(out_file, "static LIST_ARCHS: &[&str] = &[")?;
    for arch in archs {
        writeln!(out_file, r#"    "{arch}","#)?;
    }
    writeln!(out_file, "];")?;

    writeln!(out_file, "static LIST_OSES: &[&str] = &[")?;
    for os in oses {
        writeln!(out_file, r#"    "{os}","#)?;
    }
    writeln!(out_file, "];")?;

    writeln!(out_file, "static LIST_ENVS: &[&str] = &[")?;
    for env in envs {
        writeln!(out_file, r#"    "{env}","#)?;
    }
    writeln!(out_file, "];")?;

    Ok(())
}

fn main() {
    println!("cargo:rerun-if-env-changed=RUSTUP_OVERRIDE_BUILD_TRIPLE");
    println!("cargo:rerun-if-env-changed=TARGET");
    match from_build() {
        Ok(triple) => eprintln!("Computed build based partial target triple: {triple:#?}"),
        Err(s) => {
            eprintln!("Unable to parse target '{s}' as a PartialTargetTriple");
            eprintln!(
                "If you are attempting to bootstrap a new target you may need to adjust the\n\
               permitted values found in src/dist/triple.rs"
            );
            std::process::abort();
        }
    }
    let target = env::var("TARGET").unwrap();
    println!("cargo:rustc-env=TARGET={target}");

    // Generate known target triple segments.
    if let Err(e) = generate_known_triples() {
        eprintln!("Unable to generate known target triple segments: {e:?}");
        std::process::abort();
    }

    // Set linker options specific to Windows MSVC.
    let target_os = env::var("CARGO_CFG_TARGET_OS");
    let target_env = env::var("CARGO_CFG_TARGET_ENV");
    if !(target_os.as_deref() == Ok("windows") && target_env.as_deref() == Ok("msvc")) {
        return;
    }

    // # Only search system32 for DLLs
    //
    // This applies to DLLs loaded at load time. However, this setting is ignored
    // before Windows 10 RS1 (aka 1601).
    // https://learn.microsoft.com/en-us/cpp/build/reference/dependentloadflag?view=msvc-170
    println!("cargo:cargo:rustc-link-arg-bin=rustup-init=/DEPENDENTLOADFLAG:0x800");

    // # Delay load
    //
    // Delay load dlls that are not "known DLLs"[1].
    // Known DLLs are always loaded from the system directory whereas other DLLs
    // are loaded from the application directory. By delay loading the latter
    // we can ensure they are instead loaded from the system directory.
    // [1]: https://learn.microsoft.com/en-us/windows/win32/dlls/dynamic-link-library-search-order#factors-that-affect-searching
    //
    // This will work on all supported Windows versions but it relies on
    // us using `SetDefaultDllDirectories` before any libraries are loaded.
    // See also: src/bin/rustup-init.rs
    let delay_load_dlls = ["bcrypt", "powrprof", "secur32"];
    for dll in delay_load_dlls {
        println!("cargo:rustc-link-arg-bin=rustup-init=/delayload:{dll}.dll");
    }
    // When using delayload, it's necessary to also link delayimp.lib
    // https://learn.microsoft.com/en-us/cpp/build/reference/dependentloadflag?view=msvc-170
    println!("cargo:rustc-link-arg-bin=rustup-init=delayimp.lib");

    // # Turn linker warnings into errors
    //
    // Rust hides linker warnings meaning mistakes may go unnoticed.
    // Turning them into errors forces them to be displayed (and the build to fail).
    // If we do want to ignore specific warnings then `/IGNORE:` should be used.
    println!("cargo:cargo:rustc-link-arg-bin=rustup-init=/WX");
}
