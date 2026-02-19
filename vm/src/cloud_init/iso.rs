use anyhow::{Context, Result, bail};
use std::path::Path;
use tracing::{debug, info};

/// Creates NOCLOUD ISO images for cloud-init
pub struct IsoCreator {
    iso_tool_path: String,
}

impl IsoCreator {
    /// Create a new ISO creator
    pub fn new() -> Result<Self> {
        let iso_tool_path = Self::find_genisoimage()?;
        Ok(Self { iso_tool_path })
    }

    /// Find the genisoimage or xorriso binary
    fn find_genisoimage() -> Result<String> {
        // Try genisoimage first
        if let Ok(path) = Self::which("genisoimage") {
            return Ok(path);
        }

        // Fall back to xorriso
        if let Ok(path) = Self::which("xorriso") {
            return Ok(path);
        }

        // Fall back to mkisofs
        if let Ok(path) = Self::which("mkisofs") {
            return Ok(path);
        }

        bail!(
            "Neither genisoimage, xorriso, nor mkisofs found. \
             Please install one of these packages: genisoimage, xorriso, or cdrtools"
        );
    }

    /// Find a command in PATH
    fn which(command: &str) -> Result<String> {
        use std::process::Command;
        let output = Command::new("which")
            .arg(command)
            .output()
            .context(format!("Failed to execute which for {}", command))?;

        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Ok(path);
            }
        }
        bail!("{} not found in PATH", command)
    }

    /// Create NOCLOUD ISO from metadata and userdata files
    pub fn create_nocloud_iso<P: AsRef<Path>>(
        &self,
        metadata_path: P,
        userdata_path: P,
        output_iso: P,
    ) -> Result<()> {
        let metadata_path = metadata_path.as_ref();
        let userdata_path = userdata_path.as_ref();
        let output_iso = output_iso.as_ref();

        info!("Creating NOCLOUD ISO: {}", output_iso.display());

        let output_str = output_iso.to_str().context("Non-UTF8 ISO output path")?;
        let metadata_str = metadata_path.to_str().context("Non-UTF8 metadata path")?;
        let userdata_str = userdata_path.to_str().context("Non-UTF8 userdata path")?;

        let tool_name = self.tool_name();

        let mut cmd = if tool_name == "xorriso" {
            let mut cmd = std::process::Command::new(&self.iso_tool_path);
            cmd.args([
                "-as",
                "mkisofs",
                "-output",
                output_str,
                "-volid",
                "cidata",
                "-joliet",
                "-rock",
                metadata_str,
                userdata_str,
            ]);
            cmd
        } else {
            let mut cmd = std::process::Command::new(&self.iso_tool_path);
            cmd.args([
                "-output",
                output_str,
                "-volid",
                "cidata",
                "-joliet",
                "-rock",
                metadata_str,
                userdata_str,
            ]);
            cmd
        };

        debug!("Running command: {:?}", cmd);

        let output = cmd
            .output()
            .context("Failed to execute genisoimage/xorriso")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to create ISO: {}", stderr);
        }

        debug!("ISO created successfully");
        Ok(())
    }

    /// Get the name of the ISO creation tool being used
    fn tool_name(&self) -> &str {
        if self.iso_tool_path.contains("xorriso") {
            "xorriso"
        } else if self.iso_tool_path.contains("mkisofs") {
            "mkisofs"
        } else {
            "genisoimage"
        }
    }
}

impl Default for IsoCreator {
    fn default() -> Self {
        Self::new().expect("Failed to initialize IsoCreator")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_which_finds_existing_command() {
        let result = IsoCreator::which("ls");
        assert!(result.is_ok());
        assert!(!result.unwrap().is_empty());
    }

    #[test]
    fn test_which_fails_for_missing_command() {
        let result = IsoCreator::which("nonexistent_command_xyz_12345");
        assert!(result.is_err());
    }

    #[test]
    fn test_tool_name_detection() {
        let creator = IsoCreator {
            iso_tool_path: "/usr/bin/xorriso".to_string(),
        };
        assert_eq!(creator.tool_name(), "xorriso");

        let creator = IsoCreator {
            iso_tool_path: "/usr/bin/mkisofs".to_string(),
        };
        assert_eq!(creator.tool_name(), "mkisofs");

        let creator = IsoCreator {
            iso_tool_path: "/usr/bin/genisoimage".to_string(),
        };
        assert_eq!(creator.tool_name(), "genisoimage");
    }
}
