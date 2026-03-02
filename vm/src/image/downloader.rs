use super::disk::has_qcow2_magic;
use anyhow::{Context, Result, bail};
use futures::StreamExt;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use reqwest::Client;
use std::path::Path;
use std::process::Command;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tracing::{debug, info};

/// Downloads Ubuntu Cloud images with progress tracking
pub struct ImageDownloader {
    client: Client,
}

impl ImageDownloader {
    /// Create a new image downloader
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    /// Download an image from URL to the specified path
    ///
    /// Returns the number of bytes downloaded
    pub async fn download<P: AsRef<Path>>(
        &self,
        url: &str,
        dest: P,
        mp: Option<&MultiProgress>,
    ) -> Result<u64> {
        let dest = dest.as_ref();

        // Check if file already exists
        if dest.exists() {
            if Self::verify_image(dest).context("Failed to verify existing image")? {
                info!("Image already exists at: {}", dest.display());
                return Ok(dest.metadata()?.len());
            }

            std::fs::remove_file(dest).with_context(|| {
                format!("Failed to remove invalid cached image {}", dest.display())
            })?;
        }

        // Create parent directories if needed
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }

        info!("Starting download: {} -> {}", url, dest.display());

        // Start the download request
        let response = self
            .client
            .get(url)
            .send()
            .await
            .with_context(|| format!("Failed to start download from: {}", url))?
            .error_for_status()
            .with_context(|| format!("Remote server returned error while downloading {}", url))?;

        let total_size = response.content_length().unwrap_or(0);
        debug!("Total size: {} bytes", total_size);

        let temp_path = {
            let now_nanos = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            {
                Ok(duration) => duration.as_nanos().to_string(),
                Err(_) => "0".to_string(),
            };
            let temp_name = dest
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("image");
            dest.with_file_name(format!(
                ".{}-{}.{}.part",
                temp_name,
                std::process::id(),
                now_nanos
            ))
        };

        // Create progress bar
        let pb = if total_size > 0 {
            let pb = ProgressBar::new(total_size);
            let tmpl = if mp.is_some() {
                "    {spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})"
            } else {
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})"
            };
            pb.set_style(
                ProgressStyle::default_bar()
                    .template(tmpl)
                    .unwrap()
                    .progress_chars("#>-"),
            );
            pb
        } else {
            let pb = ProgressBar::new_spinner();
            let tmpl = if mp.is_some() {
                "    {spinner:.green} [{elapsed_precise}] {msg}"
            } else {
                "{spinner:.green} [{elapsed_precise}] {msg}"
            };
            pb.set_style(ProgressStyle::default_spinner().template(tmpl).unwrap());
            pb.set_message("Downloading...");
            pb
        };
        let pb = if let Some(m) = mp { m.add(pb) } else { pb };

        // Stream the response body to file
        let mut file = File::create(&temp_path)
            .await
            .with_context(|| format!("Failed to create file: {}", dest.display()))?;

        let mut stream = response.bytes_stream();
        let mut downloaded = 0u64;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("Error while downloading image")?;
            file.write_all(&chunk)
                .await
                .context("Failed to write to file")?;
            downloaded += chunk.len() as u64;
            pb.set_position(downloaded);
        }

        if mp.is_some() {
            pb.finish_and_clear();
        } else {
            pb.finish_with_message(format!("Downloaded to {}", dest.display()));
        }
        debug!("Download complete: {} bytes", downloaded);

        if total_size > 0 && downloaded != total_size {
            let _ = tokio::fs::remove_file(&temp_path).await;
            bail!(
                "Download incomplete: expected {} bytes, got {} bytes",
                total_size,
                downloaded
            );
        }

        Self::finalize_download(&temp_path, dest).await?;

        // Verify the file
        let file_size = tokio::fs::metadata(dest).await?.len();
        if !Self::verify_image(dest)
            .with_context(|| format!("Downloaded image is invalid: {}", dest.display()))?
        {
            tokio::fs::remove_file(&dest)
                .await
                .context("Failed to remove invalid downloaded image")?;
            bail!("Downloaded image failed validation: {}", dest.display());
        }

        Ok(file_size)
    }

    /// Check if an image exists and is valid
    pub fn verify_image<P: AsRef<Path>>(path: P) -> Result<bool> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(false);
        }

        let metadata = std::fs::metadata(path).context("Failed to read file metadata")?;

        // Check if file is empty
        if metadata.len() == 0 {
            return Ok(false);
        }

        if has_qcow2_magic(path)? {
            return Ok(true);
        }

        if Self::probe_with_qemu_img(path).unwrap_or(false) {
            return Ok(true);
        }

        Ok(false)
    }

    fn is_qemu_img_available() -> bool {
        Command::new("which")
            .arg("qemu-img")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    fn probe_with_qemu_img(path: &Path) -> Result<bool> {
        if !Self::is_qemu_img_available() {
            return Ok(false);
        }

        let output = Command::new("qemu-img")
            .arg("info")
            .arg(path)
            .output()
            .context("Failed to execute qemu-img")?;

        Ok(output.status.success())
    }

    async fn finalize_download(temp_path: &Path, dest: &Path) -> Result<()> {
        if dest.exists() && Self::verify_image(dest).unwrap_or(false) {
            tokio::fs::remove_file(temp_path).await.with_context(|| {
                format!(
                    "Failed to remove temporary image file after concurrent finalize: {}",
                    temp_path.display()
                )
            })?;
            return Ok(());
        }

        match tokio::fs::rename(temp_path, dest).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if Self::verify_image(dest).unwrap_or(false) {
                    tokio::fs::remove_file(temp_path).await.with_context(|| {
                        format!(
                            "Failed to remove temporary image file after concurrent finalize: {}",
                            temp_path.display()
                        )
                    })?;
                    return Ok(());
                }

                match tokio::fs::remove_file(dest).await {
                    Ok(()) => {}
                    Err(remove_err) if remove_err.kind() == std::io::ErrorKind::NotFound => {}
                    Err(remove_err) => {
                        return Err(remove_err).with_context(|| {
                            format!("Failed to overwrite existing image {}", dest.display())
                        });
                    }
                }

                match tokio::fs::rename(temp_path, dest).await {
                    Ok(()) => Ok(()),
                    Err(rename_err) if rename_err.kind() == std::io::ErrorKind::AlreadyExists => {
                        if Self::verify_image(dest).unwrap_or(false) {
                            tokio::fs::remove_file(temp_path).await.with_context(|| {
                                format!(
                                    "Failed to remove temporary image file after concurrent finalize: {}",
                                    temp_path.display()
                                )
                            })?;
                            Ok(())
                        } else {
                            let _ = tokio::fs::remove_file(temp_path).await;
                            Err(rename_err).context(format!(
                                "Failed to finalize image file: {}",
                                dest.display()
                            ))
                        }
                    }
                    Err(rename_err) => {
                        let _ = tokio::fs::remove_file(temp_path).await;
                        Err(rename_err)
                            .context(format!("Failed to finalize image file: {}", dest.display()))
                    }
                }
            }
            Err(err) => {
                let _ = tokio::fs::remove_file(temp_path).await;
                Err(err).context(format!("Failed to finalize image file: {}", dest.display()))
            }
        }
    }
}

impl Default for ImageDownloader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_verify_image_nonexistent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("does_not_exist.img");
        assert!(!ImageDownloader::verify_image(&path).unwrap());
    }

    #[test]
    fn test_verify_image_empty_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty.img");
        std::fs::write(&path, b"").unwrap();
        assert!(!ImageDownloader::verify_image(&path).unwrap());
    }

    #[test]
    fn test_verify_image_valid_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("valid.img");
        std::fs::write(&path, b"QFI\xfb").unwrap();
        assert!(ImageDownloader::verify_image(&path).unwrap());
    }

    #[tokio::test]
    async fn test_finalize_download_keeps_valid_existing_file() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("ubuntu.img");
        let temp = dir.path().join("ubuntu.img.part");
        std::fs::write(&dest, b"QFI\xfbexisting").unwrap();
        std::fs::write(&temp, b"QFI\xfbnew").unwrap();

        ImageDownloader::finalize_download(&temp, &dest)
            .await
            .unwrap();

        assert!(!temp.exists(), "temporary file should be cleaned up");
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            b"QFI\xfbexisting",
            "existing valid image should not be overwritten",
        );
    }

    #[tokio::test]
    async fn test_finalize_download_replaces_invalid_existing_file() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("ubuntu.img");
        let temp = dir.path().join("ubuntu.img.part");
        std::fs::write(&dest, b"").unwrap();
        std::fs::write(&temp, b"QFI\xfbnew").unwrap();

        ImageDownloader::finalize_download(&temp, &dest)
            .await
            .unwrap();

        assert!(!temp.exists(), "temporary file should be moved");
        assert_eq!(std::fs::read(&dest).unwrap(), b"QFI\xfbnew");
    }
}
