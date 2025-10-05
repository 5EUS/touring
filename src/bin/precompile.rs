#[cfg(target_os = "ios")]
fn main() {
    eprintln!("precompile tool is unavailable when targeting iOS");
}

#[cfg(not(target_os = "ios"))]
use std::fs;
#[cfg(not(target_os = "ios"))]
use std::path::{Path, PathBuf};

#[cfg(not(target_os = "ios"))]
use anyhow::{anyhow, Context, Result};
#[cfg(not(target_os = "ios"))]
use clap::{ArgAction, Parser};
#[cfg(not(target_os = "ios"))]
use wasmtime::{Config, Engine, OptLevel, Strategy};

/// Precompile a Wasmtime component (`.wasm`) into a precompiled artifact (`.cwasm`).
#[cfg(not(target_os = "ios"))]
#[derive(Debug, Parser)]
#[command(author, version, about = "Precompile Wasmtime components for touring plugins", long_about = None)]
struct Args {
    /// Input component path (compiled with `cargo component` / `wasm32-wasip2` target)
    #[arg(value_name = "INPUT")]
    input: PathBuf,

    /// Optional explicit output path. Defaults to replacing `.wasm` with `.cwasm`.
    #[arg(short, long, value_name = "OUTPUT")]
    output: Option<PathBuf>,

    /// Target triple for the precompiled artifact (e.g. `aarch64-apple-ios`).
    /// Defaults to the host target.
    #[arg(long, value_name = "TRIPLE")]
    target: Option<String>,

    /// Optimization level for the precompiled artifact (speed, speed_and_size, none).
    #[arg(long, value_name = "LEVEL", default_value = "speed")]
    opt_level: OptChoice,

    /// Skip copying the sibling `.toml` config next to the output artifact.
    #[arg(long, action = ArgAction::SetTrue)]
    skip_config: bool,

    /// Override the plugin configuration file that should be copied.
    #[arg(long, value_name = "CONFIG")]
    config_path: Option<PathBuf>,

    /// Optional directory to copy the resulting artifacts (and config) into.
    #[arg(long, value_name = "DIR")]
    plugins_dir: Option<PathBuf>,
}

#[cfg(not(target_os = "ios"))]
#[derive(Debug, Clone, Copy)]
enum OptChoice {
    Speed,
    SpeedAndSize,
    None,
}

#[cfg(not(target_os = "ios"))]
impl std::str::FromStr for OptChoice {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "speed" | "fast" => Ok(Self::Speed),
            "speed_and_size" | "balanced" | "s" => Ok(Self::SpeedAndSize),
            "none" | "0" => Ok(Self::None),
            other => Err(anyhow!(
                "unknown opt-level `{}` (expected speed, speed_and_size, none)",
                other
            )),
        }
    }
}

#[cfg(not(target_os = "ios"))]
impl OptChoice {
    fn to_opt_level(self) -> OptLevel {
        match self {
            OptChoice::Speed => OptLevel::Speed,
            OptChoice::SpeedAndSize => OptLevel::SpeedAndSize,
            OptChoice::None => OptLevel::None,
        }
    }
}

#[cfg(not(target_os = "ios"))]
fn main() -> Result<()> {
    let args = Args::parse();
    run(args)
}

#[cfg(not(target_os = "ios"))]
fn run(args: Args) -> Result<()> {
    let input = args
        .input
        .canonicalize()
        .with_context(|| format!("failed to resolve input path: {}", args.input.display()))?;
    if !input.exists() {
        return Err(anyhow!("input file does not exist: {}", input.display()));
    }
    if input.extension().and_then(|ext| ext.to_str()) != Some("wasm") {
        return Err(anyhow!(
            "input must be a `.wasm` component (got: {})",
            input.display()
        ));
    }

    let output = resolve_output_path(&input, args.output)?;
    let cfg_source = resolve_config_path(&input, args.config_path.as_deref());

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create output directory: {}", parent.display()))?;
    }

    let mut config = Config::new();
    config.wasm_component_model(true);
    config.async_support(true);
    config.epoch_interruption(true);
    config.strategy(Strategy::Cranelift);
    config.cranelift_opt_level(args.opt_level.to_opt_level());

    if let Some(target_triple) = args.target.as_deref() {
        config
            .target(target_triple)
            .with_context(|| format!("invalid target triple: {}", target_triple))?;
    }

    let engine = Engine::new(&config)?;
    let component_bytes = fs::read(&input)
        .with_context(|| format!("failed to read component: {}", input.display()))?;
    // Validate the component using the configured engine before precompiling.
    let serialized = engine
        .precompile_component(&component_bytes)
        .with_context(|| format!("failed to precompile component: {}", input.display()))?;
    fs::write(&output, serialized)
        .with_context(|| format!("failed to write precompiled artifact: {}", output.display()))?;

    if !args.skip_config {
        if let Some(cfg_source) = cfg_source.as_ref() {
            let cfg_output = output.with_extension("toml");
            fs::copy(cfg_source, &cfg_output).with_context(|| {
                format!(
                    "failed to copy config from {} to {}",
                    cfg_source.display(),
                    cfg_output.display()
                )
            })?;
        } else {
            eprintln!(
                "warning: no sibling .toml config found next to {}",
                input.display()
            );
        }
    }

    if let Some(dir) = args.plugins_dir {
        copy_into_plugins_dir(&output, args.skip_config, dir, cfg_source.as_ref())?;
    }

    println!("precompiled {} -> {}", input.display(), output.display());
    Ok(())
}

#[cfg(not(target_os = "ios"))]
fn resolve_output_path(input: &Path, explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    Ok(input.with_extension("cwasm"))
}

#[cfg(not(target_os = "ios"))]
fn resolve_config_path(input: &Path, override_path: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = override_path {
        return Some(path.to_path_buf());
    }
    let default = input.with_extension("toml");
    default.exists().then_some(default)
}

#[cfg(not(target_os = "ios"))]
fn copy_into_plugins_dir(
    artifact: &Path,
    skip_config: bool,
    dir: PathBuf,
    cfg_source: Option<&PathBuf>,
) -> Result<()> {
    if !dir.exists() {
        fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create plugins dir: {}", dir.display()))?;
    }
    let file_name = artifact
        .file_name()
        .ok_or_else(|| anyhow!("artifact has no file name: {}", artifact.display()))?;
    let dest_artifact = dir.join(file_name);
    fs::copy(artifact, &dest_artifact).with_context(|| {
        format!(
            "failed to copy artifact to plugins dir: {} -> {}",
            artifact.display(),
            dest_artifact.display()
        )
    })?;

    if !skip_config {
        if let Some(cfg_source) = cfg_source {
            if cfg_source.exists() {
                let cfg_name = cfg_source
                    .file_name()
                    .ok_or_else(|| anyhow!("config has no file name: {}", cfg_source.display()))?;
                let dest_cfg = dir.join(cfg_name);
                fs::copy(cfg_source, &dest_cfg).with_context(|| {
                    format!(
                        "failed to copy config to plugins dir: {} -> {}",
                        cfg_source.display(),
                        dest_cfg.display()
                    )
                })?;
            }
        }
    }

    Ok(())
}

#[cfg(all(test, not(target_os = "ios")))]
mod tests {
    use super::*;

    #[test]
    fn output_path_defaults_to_cwasm() {
        let input = PathBuf::from("/tmp/foo/bar.wasm");
        let out = resolve_output_path(&input, None).unwrap();
        assert_eq!(out, PathBuf::from("/tmp/foo/bar.cwasm"));
    }

    #[test]
    fn output_path_respects_override() {
        let input = PathBuf::from("/tmp/foo/bar.wasm");
        let override_path = PathBuf::from("/other/out.cwasm");
        let out = resolve_output_path(&input, Some(override_path.clone())).unwrap();
        assert_eq!(out, override_path);
    }

    #[test]
    fn config_path_handles_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let input = tmp.path().join("plugin.wasm");
        fs::write(&input, b"wasm").unwrap();
        assert!(resolve_config_path(&input, None).is_none());
    }

    #[test]
    fn config_path_uses_override() {
        let input = PathBuf::from("/tmp/foo/bar.wasm");
        let override_path = PathBuf::from("/cfg/custom.toml");
        let resolved = resolve_config_path(&input, Some(&override_path)).unwrap();
        assert_eq!(resolved, override_path);
    }
}
