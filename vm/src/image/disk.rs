use anyhow::{Context, Result, bail};
use std::io::{self, Read};
use std::path::Path;
use std::process::Command;
use tracing::{debug, info};

use crate::qemu::config::DiskFormat;

/// QFI\xfb magic bytes identifying a qcow2 image.
pub const QCOW2_MAGIC: [u8; 4] = [b'Q', b'F', b'I', 0xFB];

/// Check whether `path` starts with the qcow2 magic header.
pub fn has_qcow2_magic(path: &Path) -> Result<bool> {
    let mut file = std::fs::File::open(path).context("Failed to open disk image")?;
    let mut header = [0u8; 4];

    match file.read_exact(&mut header) {
        Ok(()) => Ok(header == QCOW2_MAGIC),
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(err) => Err(err).context("Failed to read disk image header"),
    }
}

pub fn detect_disk_format(path: &Path) -> Result<DiskFormat> {
    if has_qcow2_magic(path)? {
        Ok(DiskFormat::Qcow2)
    } else {
        Ok(DiskFormat::Raw)
    }
}

pub fn copy_disk_image(src: &Path, dest: &Path, disk_size: Option<&str>) -> Result<()> {
    info!(
        "Copying disk image: {} -> {}",
        src.display(),
        dest.display()
    );

    let src_str = src.to_str().context("Non-UTF8 source image path")?;
    let dest_str = dest.to_str().context("Non-UTF8 destination image path")?;

    let output = Command::new("qemu-img")
        .args([
            "create", "-f", "qcow2", "-F", "qcow2", "-b", src_str, dest_str,
        ])
        .output();

    let created = match output {
        Ok(out) => {
            if out.status.success() {
                info!("Created qcow2 backing image");
                true
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                debug!("qemu-img create failed: {}, falling back to cp", stderr);
                false
            }
        }
        Err(e) => {
            debug!("qemu-img not available: {}, falling back to cp", e);
            false
        }
    };

    if !created {
        std::fs::copy(src, dest).context("Failed to copy disk image")?;

        info!("Disk image copied successfully");
    }

    if let Some(size) = disk_size {
        let size = size.trim();
        if !size.is_empty() {
            info!("Resizing disk image to {}", size);
            resize_disk_image(dest, size)?;
        }
    }

    Ok(())
}

fn resize_disk_image(path: &Path, size: &str) -> Result<()> {
    let path_str = path.to_str().context("Non-UTF8 disk image path")?;

    let output = Command::new("qemu-img")
        .args(["resize", path_str, size])
        .output()
        .context("Failed to execute qemu-img for disk resize")?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!("qemu-img resize failed: {}", stderr.trim());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::str;

    fn qemu_img_available() -> bool {
        Command::new("which")
            .arg("qemu-img")
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn qemu_info_virtual_size_bytes(path: &Path) -> Result<u64> {
        let path = path.to_str().context("Invalid disk path")?;
        let output = Command::new("qemu-img")
            .args(["info", path])
            .output()
            .with_context(|| format!("Failed to run qemu-img info for {}", path))?;

        if !output.status.success() {
            bail!("qemu-img info failed for {}", path);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if let Some(rest) = line.trim_start().strip_prefix("virtual size:") {
                let bytes_text = rest
                    .split('(')
                    .nth(1)
                    .context("Failed to parse virtual size bytes from qemu-img output")?
                    .split_whitespace()
                    .next()
                    .context("Failed to parse virtual size bytes token")?;
                return bytes_text
                    .parse()
                    .with_context(|| format!("Invalid virtual size token: {}", bytes_text));
            }
        }

        bail!("virtual size not found in qemu-img output");
    }

    #[test]
    fn test_detect_disk_format_qcow2() -> Result<()> {
        let file = tempfile::NamedTempFile::new().context("Failed to create temp file")?;
        file.as_file()
            .write_all(&[b'Q', b'F', b'I', 0xFB])
            .context("Failed to write qcow2 magic")?;

        assert!(matches!(
            detect_disk_format(file.path())?,
            DiskFormat::Qcow2
        ));

        Ok(())
    }

    #[test]
    fn test_detect_disk_format_raw() -> Result<()> {
        let file = tempfile::NamedTempFile::new().context("Failed to create temp file")?;
        file.as_file()
            .write_all(b"this is not qcow2")
            .context("Failed to write sample raw bytes")?;

        assert!(matches!(detect_disk_format(file.path())?, DiskFormat::Raw));

        Ok(())
    }

    #[test]
    fn test_copy_disk_image_resizes_disk_size() -> Result<()> {
        if !qemu_img_available() {
            eprintln!("qemu-img not available; skipping disk resize test.");
            return Ok(());
        }

        let temp_dir = tempfile::tempdir()?;
        let src = temp_dir.path().join("base.qcow2");
        let dst = temp_dir.path().join("vm.qcow2");

        let src_str = src.to_str().context("Invalid source path")?;
        let out = Command::new("qemu-img")
            .args(["create", "-f", "qcow2", src_str, "4M"])
            .output()
            .context("Failed to create base qcow2 image")?;
        if !out.status.success() {
            bail!(
                "Failed to create base qcow2 image: {}",
                str::from_utf8(&out.stderr).unwrap_or("invalid stderr")
            );
        }

        let base_size = qemu_info_virtual_size_bytes(&src)?;
        copy_disk_image(&src, &dst, Some("+2M"))?;
        let resized_size = qemu_info_virtual_size_bytes(&dst)?;

        assert!(
            resized_size > base_size,
            "resized image size {} is not greater than base {}",
            resized_size,
            base_size
        );

        Ok(())
    }
}
