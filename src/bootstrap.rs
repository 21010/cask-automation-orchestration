use anyhow::{Context, Result};
use directories::BaseDirs;
use indicatif::{ProgressBar, ProgressStyle};
use std::fs::{self, File};
use std::io::{self, Read, Write}; // Fixed: Added Read
use std::path::{Path, PathBuf};

const UV_VERSION: &str = "0.9.28";
const BASE_URL: &str = "https://github.com/astral-sh/uv/releases/download";

pub struct Engine {
    pub path: PathBuf,
}

impl Engine {
    pub fn ensure() -> Result<Self> {
        let base_dirs = BaseDirs::new().context("Could not determine home directory")?;
        let bin_dir = base_dirs.home_dir().join(".cask").join("bin");
        
        if !bin_dir.exists() {
            fs::create_dir_all(&bin_dir)?;
        }

        let uv_filename = if cfg!(windows) { "uv.exe" } else { "uv" };
        let uv_path = bin_dir.join(uv_filename);

        if uv_path.exists() {
            return Ok(Self { path: uv_path });
        }

        println!("Engine missing. Bootstrapping CASK...");
        download_and_unpack(UV_VERSION, &bin_dir)?;

        if !uv_path.exists() {
            anyhow::bail!("Download completed but binary missing at {:?}", uv_path);
        }

        println!("Engine ready.");
        Ok(Self { path: uv_path })
    }
}

fn download_and_unpack(version: &str, target_dir: &Path) -> Result<()> {
    let (os, arch, ext) = detect_platform()?;
    
    let asset_name = format!("uv-{}-{}.{}", arch, os, ext);
    let url = format!("{}/{}/{}", BASE_URL, version, asset_name);

    println!("   Downloading from: {}", url);

    let client = reqwest::blocking::Client::new();
    let mut response = client.get(&url).send()?;
    let total_size = response.content_length().unwrap_or(0);
    
    let pb = ProgressBar::new(total_size);
    pb.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")?
        .progress_chars("#>-"));

    let mut temp_archive = tempfile::tempfile()?;
    let mut downloaded: u64 = 0;
    let mut buf = [0; 8192];
    
    // This loop requires `use std::io::Read;`
    loop {
        let n = response.read(&mut buf)?;
        if n == 0 { break; }
        temp_archive.write_all(&buf[..n])?;
        downloaded += n as u64;
        pb.set_position(downloaded);
    }
    pb.finish_with_message("Download complete");

    use std::io::Seek;
    temp_archive.seek(io::SeekFrom::Start(0))?;

    println!("   Unpacking...");

    if ext == "zip" {
        let mut archive = zip::ZipArchive::new(temp_archive)?;
        for i in 0..archive.len() {
            let mut file = archive.by_index(i)?;
            let outpath = match file.enclosed_name() {
                Some(p) => p,
                None => continue,
            };

            if let Some(fname) = outpath.file_name() {
                let fname_str = fname.to_string_lossy();
                if fname_str == "uv" || fname_str == "uv.exe" {
                    let mut outfile = File::create(target_dir.join(fname))?;
                    io::copy(&mut file, &mut outfile)?;
                }
            }
        }
    } else {
        let tar = flate2::read::GzDecoder::new(temp_archive);
        let mut archive = tar::Archive::new(tar);
        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?;
            if let Some(fname) = path.file_name() {
                if fname == "uv" {
                    let mut outfile = File::create(target_dir.join("uv"))?;
                    io::copy(&mut entry, &mut outfile)?;
                }
            }
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let uv_bin = target_dir.join("uv");
        if uv_bin.exists() {
            fs::set_permissions(&uv_bin, fs::Permissions::from_mode(0o755))?;
        }
    }

    Ok(())
}

fn detect_platform() -> Result<(&'static str, &'static str, &'static str)> {
    let os = if cfg!(target_os = "windows") { "pc-windows-msvc" }
             else if cfg!(target_os = "macos") { "apple-darwin" }
             else if cfg!(target_os = "linux") { "unknown-linux-gnu" }
             else { anyhow::bail!("Unsupported OS") };

    let arch = if cfg!(target_arch = "x86_64") { "x86_64" }
               else if cfg!(target_arch = "aarch64") { "aarch64" }
               else { anyhow::bail!("Unsupported CPU Architecture") };

    let ext = if cfg!(target_os = "windows") { "zip" } else { "tar.gz" };

    Ok((os, arch, ext))
}