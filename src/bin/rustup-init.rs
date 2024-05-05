//! The main Rustup command-line interface
//!
//! The rustup binary is a chimera, changing its behavior based on the
//! name of the binary. This is used most prominently to enable
//! Rustup's tool 'proxies' - that is, rustup itself and the rustup
//! proxies are the same binary: when the binary is called 'rustup' or
//! 'rustup.exe' it offers the Rustup command-line interface, and
//! when it is called 'rustc' it behaves as a proxy to 'rustc'.
//!
//! This scheme is further used to distinguish the Rustup installer,
//! called 'rustup-init', which is again just the rustup binary under a
//! different name.

#![recursion_limit = "1024"]

use anyhow::{anyhow, Result};
use cfg_if::cfg_if;
// Public macros require availability of the internal symbols
use rs_tracing::{
    close_trace_file, close_trace_file_internal, open_trace_file, trace_to_file_internal,
};

use rustup::cli::proxy_mode;
use rustup::cli::rustup_mode;
#[cfg(windows)]
use rustup::cli::self_update;
use rustup::cli::setup_mode;
use rustup::currentprocess::{process, varsource::VarSource, with, OSProcess};
use rustup::env_var::RUST_RECURSION_COUNT_MAX;
use rustup::is_proxyable_tools;
use rustup::utils::utils;
use rustup::{cli::common, currentprocess::filesource::StderrSource};

fn main() {
    #[cfg(windows)]
    pre_rustup_main_init();

    let process = OSProcess::default();
    with(process.into(), || match maybe_trace_rustup() {
        Err(e) => {
            common::report_error(&e);
            std::process::exit(1);
        }
        Ok(utils::ExitCode(c)) => std::process::exit(c),
    });
}

fn maybe_trace_rustup() -> Result<utils::ExitCode> {
    use std::time::Duration;

    use tracing_subscriber::{fmt, layer::SubscriberExt, EnvFilter, Layer, Registry};

    let curr_process = process();
    let has_ansi = curr_process.stderr().is_a_tty();

    // Background submission requires a runtime, and since we're probably
    // going to want async eventually, we just use tokio.
    let threaded_rt = tokio::runtime::Runtime::new()?;

    let result = threaded_rt.block_on(async move {
        #[cfg(feature = "otel")]
        let telemetry = {
            use opentelemetry::{global, KeyValue};
            use opentelemetry_otlp::WithExportConfig;
            use opentelemetry_sdk::{
                propagation::TraceContextPropagator,
                trace::{self, Sampler},
                Resource,
            };

            global::set_text_map_propagator(TraceContextPropagator::new());

            let tracer = opentelemetry_otlp::new_pipeline()
                .tracing()
                .with_exporter(
                    opentelemetry_otlp::new_exporter()
                        .tonic()
                        .with_timeout(Duration::from_secs(3)),
                )
                .with_trace_config(
                    trace::config()
                        .with_sampler(Sampler::AlwaysOn)
                        .with_resource(Resource::new(vec![KeyValue::new(
                            "service.name",
                            "rustup",
                        )])),
                )
                .install_batch(opentelemetry_sdk::runtime::Tokio)?;
            let env_filter = EnvFilter::try_from_default_env().unwrap_or(EnvFilter::new("INFO"));
            tracing_opentelemetry::layer()
                .with_tracer(tracer)
                .with_filter(env_filter)
        };
        let console_logger = {
            let is_verbose = curr_process.var_os("RUST_LOG").is_some();
            let logger = fmt::layer()
                .with_writer(move || curr_process.stderr())
                .with_ansi(has_ansi);
            if is_verbose {
                let env_filter =
                    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("INFO"));
                logger.compact().with_filter(env_filter).boxed()
            } else {
                // Receive log lines from Rustup only.
                let env_filter = EnvFilter::new("rustup=DEBUG");
                logger
                    .event_format(rustup::cli::log::EventFormatter)
                    .with_filter(env_filter)
                    .boxed()
            }
        };
        let subscriber = {
            #[cfg(feature = "otel")]
            {
                Registry::default().with(console_logger).with(telemetry)
            }
            #[cfg(not(feature = "otel"))]
            {
                Registry::default().with(console_logger)
            }
        };
        tracing::subscriber::set_global_default(subscriber)?;
        let result = run_rustup();
        // We're tracing, so block until all spans are exported.
        #[cfg(feature = "otel")]
        opentelemetry::global::shutdown_tracer_provider();
        result
    });
    // default runtime behaviour is to block until nothing is running;
    // instead we supply a timeout, as we're either already errored and are
    // reporting back without care for lost threads etc... or everything
    // completed.
    threaded_rt.shutdown_timeout(Duration::from_millis(5));
    result
}

// FIXME: Make `tracing::instrument` always run
#[cfg_attr(feature = "otel", tracing::instrument)]
fn run_rustup() -> Result<utils::ExitCode> {
    if let Ok(dir) = process().var("RUSTUP_TRACE_DIR") {
        open_trace_file!(dir)?;
    }
    let result = run_rustup_inner();
    if process().var("RUSTUP_TRACE_DIR").is_ok() {
        close_trace_file!();
    }
    result
}

#[cfg_attr(feature = "otel", tracing::instrument(err))]
fn run_rustup_inner() -> Result<utils::ExitCode> {
    // Guard against infinite proxy recursion. This mostly happens due to
    // bugs in rustup.
    do_recursion_guard()?;

    // Before we do anything else, ensure we know where we are and who we
    // are because otherwise we cannot proceed usefully.
    utils::current_dir()?;
    utils::current_exe()?;

    match process().name().as_deref() {
        Some("rustup") => rustup_mode::main(),
        Some(n) if n.starts_with("rustup-setup") || n.starts_with("rustup-init") => {
            // NB: The above check is only for the prefix of the file
            // name. Browsers rename duplicates to
            // e.g. rustup-setup(2), and this allows all variations
            // to work.
            setup_mode::main()
        }
        Some(n) if n.starts_with("rustup-gc-") => {
            // This is the final uninstallation stage on windows where
            // rustup deletes its own exe
            cfg_if! {
                if #[cfg(windows)] {
                    self_update::complete_windows_uninstall()
                } else {
                    unreachable!("Attempted to use Windows-specific code on a non-Windows platform. Aborting.")
                }
            }
        }
        Some(n) => {
            is_proxyable_tools(n)?;
            proxy_mode::main(n)
        }
        None => {
            // Weird case. No arg0, or it's unparsable.
            Err(rustup::cli::errors::CLIError::NoExeName.into())
        }
    }
}

fn do_recursion_guard() -> Result<()> {
    let recursion_count = process()
        .var("RUST_RECURSION_COUNT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    if recursion_count > RUST_RECURSION_COUNT_MAX {
        return Err(anyhow!("infinite recursion detected"));
    }

    Ok(())
}

/// Windows pre-main security mitigations.
///
/// This attempts to defend against malicious DLLs that may sit alongside
/// rustup-init in the user's download folder.
#[cfg(windows)]
pub fn pre_rustup_main_init() {
    use winapi::um::libloaderapi::{SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_SYSTEM32};
    // Default to loading delay loaded DLLs from the system directory.
    // For DLLs loaded at load time, this relies on the `delayload` linker flag.
    // This is only necessary prior to Windows 10 RS1. See build.rs for details.
    unsafe {
        let result = SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32);
        // SetDefaultDllDirectories should never fail if given valid arguments.
        // But just to be safe and to catch mistakes, assert that it succeeded.
        assert_ne!(result, 0);
    }
}
