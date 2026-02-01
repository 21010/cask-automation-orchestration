mod bootstrap;
mod config;

use clap::{Parser, Subcommand};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use directories::BaseDirs;
use anyhow::{Context, Result};
use colored::*;
use dotenvy; 

#[derive(Parser)]
#[command(name = "cask")]
#[command(about = "The High-Performance RPA Environment Manager", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a new Cask project
    Init {
        /// Project name (defaults to current folder name)
        #[arg(short, long)]
        name: Option<String>,
    },
    /// Execute a command in the isolated environment
    Run {
        #[arg(short, long, default_value = "cask.yaml")]
        config: PathBuf,

        /// The command to run (e.g. "robot.py" or "-m robocorp.tasks ...")
        /// We allow hyphens so you can pass flags like "-m" or "--verbose" to Python
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Freeze dependencies into a cask.lock file
    Lock {
        #[arg(short, long, default_value = "cask.yaml")]
        config: PathBuf,
    },
    /// Destroys all environments to reclaim disk space
    Clean {
        /// Skip confirmation prompt
        #[arg(short, long)]
        force: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // 0. Handle Init (No engine needed yet)
    if let Commands::Init { name } = &cli.command {
        return init_project(name.clone());
    }

    // 1. Ensure the engine (uv) is present before doing anything else
    let engine = bootstrap::Engine::ensure()?;

    match &cli.command {
        Commands::Init { .. } => unreachable!(), // Handled above

        Commands::Clean { force } => {
            clean_holotree(*force)?;
        }

        Commands::Lock { config } => {
            lock_dependencies(&engine.path, config)?;
        }

        Commands::Run { config, args } => {
            // A. Resolve Project Root (for .env and relative paths)
            let project_root = config.parent()
                .map(|p| if p.as_os_str().is_empty() { Path::new(".") } else { p })
                .unwrap_or(Path::new("."));

            // B. Check for Lockfile & Auto-Update if Stale (Drift Detection)
            let lock_path = config.with_file_name("cask.lock");
            
            if config.exists() && lock_path.exists() {
                let yaml_meta = fs::metadata(config)?;
                let lock_meta = fs::metadata(&lock_path)?;

                if yaml_meta.modified()? > lock_meta.modified()? {
                    println!("{} Dependency drift detected (cask.yaml is newer).", "🔄".yellow());
                    lock_dependencies(&engine.path, config)?;
                }
            }

            // C. Determine Effective Configuration (Lock vs YAML)
            let (_, effective_config) = if lock_path.exists() {
                println!("{} Found cask.lock. Enforcing Strict Mode.", "🛡️".green());
                (true, lock_path.as_path())
            } else {
                println!("{} No lockfile found. Using loose dependencies.", "⚠️".yellow());
                (false, config.as_path())
            };

            // D. Load Blueprint (We always need this for Metadata & Python Version)
            if !config.exists() {
                anyhow::bail!("Config file not found: {:?}", config);
            }
            let blueprint = config::Blueprint::load(config)?;
            
            if let Some(name) = &blueprint.name {
                println!("🤖 Project: {}", name.cyan().bold());
            }
            if let Some(desc) = &blueprint.description {
                println!("📄 {}", desc.italic());
            }

            // E. Resolve Holotree Path
            let base_dirs = BaseDirs::new().context("No home dir")?;
            let holotree_root = base_dirs.home_dir().join(".cask").join("holotree");
            
            // F. Calculate Identity (Content-Addressable Hash)
            let env_hash = calculate_hash(effective_config, &blueprint.python)?;
            let env_path = holotree_root.join(&env_hash);

            println!("{} Identity: {} (Python {})", "🆔".blue(), env_hash, blueprint.python);

            // G. Build (if missing, with Self-Healing)
            if !env_path.exists() {
                println!("{} Building Holotree node...", "🔨".yellow());
                if let Err(e) = build_env(&engine.path, &env_path, effective_config, &blueprint.python) {
                    eprintln!("{} Build failed. Cleaning up...", "💥".red());
                    let _ = fs::remove_dir_all(&env_path); // Prevent zombie envs
                    return Err(e);
                }
            } else {
                println!("{} Using cached environment.", "⚡".green());
            }

            // H. Execute Payload
            run_task(&env_path, args, project_root)?;
        }
    }

    Ok(())
}

// --- CORE LOGIC ---

fn init_project(name_opt: Option<String>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let config_path = cwd.join("cask.yaml");

    if config_path.exists() {
        anyhow::bail!("cask.yaml already exists in this directory.");
    }

    let name = name_opt.or_else(|| {
        cwd.file_name()
           .and_then(|n| n.to_str())
           .map(|s| s.to_string())
    }).unwrap_or_else(|| "my-robot".to_string());

    let template = format!(
r#"name: "{}"
description: "New automation project"
python: "3.11"

dependencies:
  - robocorp-tasks
  - requests
"#, name);

    fs::write(&config_path, template)?;
    
    let task_path = cwd.join("robot.py");
    if !task_path.exists() {
        let robot_code = r#"from robocorp.tasks import task
import os

@task
def my_task():
    print(f"Hello from Cask! API_KEY present: {'API_KEY' in os.environ}")
"#;
        fs::write(&task_path, robot_code)?;
    }

    println!("{} Initialized new project: {}", "✨".green(), name);
    println!("   Run it with: cask run -- -m robocorp.tasks run robot.py");

    Ok(())
}

fn calculate_hash(file_path: &Path, python_version: &str) -> Result<String> {
    let content = fs::read(file_path).with_context(|| format!("Failed to read {:?}", file_path))?;
    
    let mut hasher = Sha256::new();
    hasher.update(python_version.as_bytes());
    hasher.update(&content);
    hasher.update(std::env::consts::OS.as_bytes()); // Mix in OS to prevent sharing binary envs
    
    let result = hasher.finalize();
    Ok(hex::encode(result)[..16].to_string())
}

fn lock_dependencies(uv: &Path, config_path: &Path) -> Result<()> {
    println!("{} Locking dependencies...", "🔒".cyan());

    let blueprint = config::Blueprint::load(config_path)?;
    let temp_reqs = config_path.with_extension("tmp");
    fs::write(&temp_reqs, blueprint.to_requirements_txt())?;

    let lock_file = config_path.with_file_name("cask.lock");

    let status = Command::new(uv)
        .arg("pip")
        .arg("compile")
        .arg(&temp_reqs)
        .arg("-o")
        .arg(&lock_file)
        .arg("--python")
        .arg(&blueprint.python)
        .status()?;

    let _ = fs::remove_file(temp_reqs);

    if !status.success() {
        anyhow::bail!("Failed to lock dependencies");
    }

    println!("{} Locked to {:?}", "✅".green(), lock_file);
    Ok(())
}

fn build_env(uv: &Path, env_path: &Path, req_file: &Path, python_version: &str) -> Result<()> {
    fs::create_dir_all(env_path)?;

    // A. Create Venv
    println!("{} Fetching Python {}...", "🐍".magenta(), python_version);
    let status = Command::new(uv)
        .arg("venv")
        .arg(".venv")
        .arg("--python")
        .arg(python_version)
        .current_dir(env_path)
        .status()?;
    
    if !status.success() { anyhow::bail!("Failed to create venv"); }

    // B. Install Dependencies
    println!("{} Installing dependencies...", "📦".magenta());
    
    let is_yaml = req_file.extension().and_then(|s| s.to_str()) == Some("yaml");
    
    let install_target = if is_yaml {
        // Convert YAML -> temp requirements.txt
        let bp = config::Blueprint::load(req_file)?;
        let temp_req = env_path.join("temp_reqs.txt");
        fs::write(&temp_req, bp.to_requirements_txt())?;
        temp_req
    } else {
        // Lockfile: Must use absolute path because we change CWD
        fs::canonicalize(req_file)?
    };

    let status = Command::new(uv)
        .args(["pip", "install", "-r"])
        .arg(&install_target)
        .current_dir(env_path)
        .status()?;

    if is_yaml {
        let _ = fs::remove_file(&install_target);
    }

    if !status.success() { anyhow::bail!("Failed to install dependencies"); }

    Ok(())
}

fn run_task(env_path: &Path, args: &[String], project_root: &Path) -> Result<()> {
    let venv_root = env_path.join(".venv");
    
    #[cfg(target_os = "windows")]
    let python = venv_root.join("Scripts").join("python.exe");
    #[cfg(not(target_os = "windows"))]
    let python = venv_root.join("bin").join("python");

    let display_cmd = args.join(" ");
    println!("{} Launching payload: '{}' \n", "🚀".red(), display_cmd);

    let mut command = Command::new(python);
    command.args(args);
    command.env("VIRTUAL_ENV", &venv_root);

    // .ENV Injection
    let dotenv_path = project_root.join(".env");
    if dotenv_path.exists() {
        println!("{} Loading secrets from .env", "🔑".yellow());
        for item in dotenvy::from_path_iter(&dotenv_path)? {
            let (key, val) = item?;
            command.env(key, val);
        }
    }

    let status = command.status()?;

    if !status.success() {
        anyhow::bail!("Process exited with error");
    }
    Ok(())
}

fn clean_holotree(force: bool) -> Result<()> {
    let base_dirs = BaseDirs::new().context("No home dir")?;
    let holotree_root = base_dirs.home_dir().join(".cask").join("holotree");

    if !holotree_root.exists() {
        println!("{} Holotree is already empty.", "✨".green());
        return Ok(());
    }

    if !force {
        let count = fs::read_dir(&holotree_root)?.count();
        println!("{} Warning: This will delete {} environment(s).", "⚠️".yellow(), count);
        print!("   Are you sure? [y/N]: ");
        use std::io::Write;
        std::io::stdout().flush()?;

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        
        if input.trim().to_lowercase() != "y" {
            println!("   Aborted.");
            return Ok(());
        }
    }

    println!("{} Destroying Holotree...", "🔥".red());
    fs::remove_dir_all(&holotree_root)?;
    println!("{} System reset complete.", "✨".green());

    Ok(())
}