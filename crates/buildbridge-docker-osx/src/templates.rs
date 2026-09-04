//! Machine templates: a prepared machine's disk saved once on this host, compressed, so that
//! new machines are cloned from it in seconds instead of reinstalled in an hour.
//!
//! A template is a standalone qcow2 image plus the NVRAM and, when the source had it, the
//! install media, in a directory of its own. A clone's disk is a copy-on-write overlay whose
//! backing file is the template's image at the fixed path every container binds the template
//! directory to, read-only; the overlay costs nothing until the clone writes. A template is
//! therefore never modified while a clone exists, and macOS is never shipped anywhere: the
//! template is the user's own installation, on the user's own host.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use ts_rs::TS;

use crate::disk::{
    DISK_BASESYSTEM_NAME, DISK_IMAGE_NAME, DISK_NVRAM_NAME, MachineDisk, THROWAWAY_DISK_DIR,
    available_bytes, non_empty_file, required_free_bytes, restrict_directory, shut_down_guest,
    validate_bind_path,
};
use crate::{
    ContainerState, DOCKER_IMAGE, ProviderError, TrackedCommand, clean_output, inspect_container,
    run_docker,
};

/// Where every container that runs a clone binds its template directory, read-only. The
/// overlay's backing path is recorded relative to this, so it must never change.
pub const CONTAINER_TEMPLATE_DIR: &str = "/buildbridge-template";
const THROWAWAY_SOURCE_DIR: &str = "/buildbridge-source";
const THROWAWAY_OUTPUT_DIR: &str = "/buildbridge-output";
const COMPRESS_POLL: Duration = Duration::from_millis(500);

/// The host directory holding one template's files.
#[derive(Debug, Clone)]
pub struct MachineTemplateFiles {
    dir: PathBuf,
}

impl MachineTemplateFiles {
    pub fn new(dir: &Path) -> Result<Self, ProviderError> {
        validate_bind_path(dir, "template directory")?;

        Ok(Self {
            dir: dir.to_path_buf(),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn image(&self) -> PathBuf {
        self.dir.join(DISK_IMAGE_NAME)
    }

    pub fn nvram(&self) -> PathBuf {
        self.dir.join(DISK_NVRAM_NAME)
    }

    pub fn basesystem(&self) -> PathBuf {
        self.dir.join(DISK_BASESYSTEM_NAME)
    }

    /// The image and NVRAM exist and are not empty; the install media is optional.
    pub fn ready(&self) -> bool {
        non_empty_file(&self.image()) && non_empty_file(&self.nvram())
    }

    /// Bytes on disk, for the list and the space check before a save.
    pub fn size_bytes(&self) -> u64 {
        [self.image(), self.nvram(), self.basesystem()]
            .iter()
            .filter_map(|path| fs::metadata(path).ok())
            .map(|metadata| metadata.len())
            .sum()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum TemplateSavePhase {
    ShuttingDown,
    CheckingSpace,
    CompressingDisk,
    CopyingFiles,
    Completed,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct TemplateSaveProgress {
    pub phase: TemplateSavePhase,
    #[ts(type = "number")]
    pub elapsed_seconds: u64,
    pub detail: String,
    /// How far `qemu-img` is through the compression, when it says.
    pub percent: Option<u8>,
}

/// `qemu-img convert -p -c` in a throwaway container: the source read-only, the output
/// directory writable, compressed standalone qcow2 out. A source that is itself a clone reads
/// through its own template, bound where its overlay expects it, and the conversion flattens
/// the chain. The image's own tool, so the format is what the image boots.
pub(crate) fn template_compress_args(source: &MachineDisk, output_dir: &Path) -> Vec<String> {
    let mut args = vec![
        "run".to_string(),
        "--rm".to_string(),
        format!(
            "--volume={}:{THROWAWAY_SOURCE_DIR}:ro",
            source.dir().display()
        ),
        format!(
            "--volume={}:{THROWAWAY_OUTPUT_DIR}:rw",
            output_dir.display()
        ),
    ];
    if let Some(template_dir) = source.template_dir() {
        args.push(format!(
            "--volume={}:{CONTAINER_TEMPLATE_DIR}:ro",
            template_dir.display()
        ));
    }
    args.extend([
        DOCKER_IMAGE.to_string(),
        "qemu-img".to_string(),
        "convert".to_string(),
        "-p".to_string(),
        "-c".to_string(),
        "-O".to_string(),
        "qcow2".to_string(),
        format!("{THROWAWAY_SOURCE_DIR}/{DISK_IMAGE_NAME}"),
        format!("{THROWAWAY_OUTPUT_DIR}/{DISK_IMAGE_NAME}.part"),
    ]);

    args
}

/// The clone's overlay: a new qcow2 whose backing file is the template image at the path the
/// machine's container will bind the template to.
pub(crate) fn template_clone_args(template_dir: &Path, disk_dir: &Path) -> Vec<String> {
    vec![
        "run".to_string(),
        "--rm".to_string(),
        format!(
            "--volume={}:{CONTAINER_TEMPLATE_DIR}:ro",
            template_dir.display()
        ),
        format!("--volume={}:{THROWAWAY_DISK_DIR}:rw", disk_dir.display()),
        DOCKER_IMAGE.to_string(),
        "qemu-img".to_string(),
        "create".to_string(),
        "-f".to_string(),
        "qcow2".to_string(),
        "-b".to_string(),
        format!("{CONTAINER_TEMPLATE_DIR}/{DISK_IMAGE_NAME}"),
        "-F".to_string(),
        "qcow2".to_string(),
        format!("{THROWAWAY_DISK_DIR}/{DISK_IMAGE_NAME}"),
    ]
}

/// `qemu-img -p` prints `    (12.34/100%)` after a carriage return; the last one wins.
pub(crate) fn parse_convert_progress(output: &str) -> Option<u8> {
    output
        .rsplit('(')
        .find_map(|part| {
            let number = part.split('/').next()?.trim();
            number.parse::<f64>().ok()
        })
        .map(|value| value.clamp(0.0, 100.0) as u8)
}

/// Saves a machine's disk as a template. macOS is asked to shut down first, so the copy is of
/// a consistent disk; the machine stays stopped afterwards. The compressed image lands under
/// a `.part` name until it is complete, so a save that stops halfway leaves nothing a clone
/// could pick up. Returns the template's size on disk.
pub fn save_template<F>(
    container_name: &str,
    qmp_socket: &Path,
    source: &MachineDisk,
    template: &MachineTemplateFiles,
    mut on_progress: F,
) -> Result<u64, ProviderError>
where
    F: FnMut(TemplateSaveProgress),
{
    if !non_empty_file(&source.image()) {
        return Err(ProviderError::Identity(
            "this machine keeps no macOS disk on this host yet".to_string(),
        ));
    }
    let started = Instant::now();
    let mut report = |phase: TemplateSavePhase, detail: &str, percent: Option<u8>| {
        on_progress(TemplateSaveProgress {
            phase,
            elapsed_seconds: started.elapsed().as_secs(),
            detail: detail.to_string(),
            percent,
        });
    };

    let (state, _, _) = inspect_container(container_name)?;
    if matches!(
        state,
        ContainerState::Running | ContainerState::Paused | ContainerState::Restarting
    ) {
        report(
            TemplateSavePhase::ShuttingDown,
            "Asking macOS to shut down so the disk is copied at rest",
            None,
        );
        shut_down_guest(container_name, qmp_socket, &mut |detail| {
            report(TemplateSavePhase::ShuttingDown, detail, None)
        })?;
    }

    fs::create_dir_all(template.dir())
        .map_err(|error| ProviderError::Identity(error.to_string()))?;
    restrict_directory(template.dir())?;
    report(
        TemplateSavePhase::CheckingSpace,
        "Checking free space for the compressed copy",
        None,
    );
    let allocated = {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(source.image())
            .map(|metadata| metadata.blocks().saturating_mul(512))
            .map_err(|error| ProviderError::Identity(error.to_string()))?
    };
    let needed = required_free_bytes(allocated);
    let available = available_bytes(template.dir())?;
    if available < needed {
        return Err(ProviderError::Identity(format!(
            "the templates directory has {} free and the copy may need up to {}; free some space and try again",
            crate::disk::format_gib(available),
            crate::disk::format_gib(needed)
        )));
    }

    let partial = template.dir().join(format!("{DISK_IMAGE_NAME}.part"));
    let _ = fs::remove_file(&partial);
    report(
        TemplateSavePhase::CompressingDisk,
        "Compressing the macOS disk into the template",
        Some(0),
    );
    let mut child = Command::new("docker")
        .args(template_compress_args(source, template.dir()))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .tracked_spawn()
        .map_err(|error| ProviderError::DockerUnavailable(error.to_string()))?;
    let percent = Arc::new(Mutex::new(None::<u8>));
    let reader = child.stdout.take().map(|mut stdout| {
        let percent = Arc::clone(&percent);
        thread::spawn(move || {
            use std::io::Read;
            let mut buffer = [0u8; 4096];
            let mut tail = String::new();
            while let Ok(read) = stdout.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                tail.push_str(&String::from_utf8_lossy(&buffer[..read]));
                if tail.len() > 512 {
                    tail = tail.split_off(tail.len() - 256);
                }
                if let Some(value) = parse_convert_progress(&tail)
                    && let Ok(mut current) = percent.lock()
                {
                    *current = Some(value);
                }
            }
        })
    });
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                let current = percent.lock().ok().and_then(|value| *value);
                report(
                    TemplateSavePhase::CompressingDisk,
                    "Compressing the macOS disk into the template",
                    current,
                );
                thread::sleep(COMPRESS_POLL);
            }
            Err(error) => {
                let _ = fs::remove_file(&partial);
                return Err(ProviderError::DockerUnavailable(error.to_string()));
            }
        }
    };
    if let Some(reader) = reader {
        let _ = reader.join();
    }
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = std::io::Read::read_to_string(&mut pipe, &mut stderr);
    }
    if !status.success() {
        let _ = fs::remove_file(&partial);
        return Err(ProviderError::DockerCommand {
            operation: "template compression",
            message: clean_output(stderr.as_bytes()),
        });
    }
    if !non_empty_file(&partial) {
        let _ = fs::remove_file(&partial);
        return Err(ProviderError::DockerCommand {
            operation: "template compression",
            message: "the compressed image came back empty".to_string(),
        });
    }
    fs::rename(&partial, template.image())
        .map_err(|error| ProviderError::Identity(error.to_string()))?;

    report(
        TemplateSavePhase::CopyingFiles,
        "Copying the NVRAM and install media",
        Some(100),
    );
    fs::copy(source.nvram(), template.nvram())
        .map_err(|error| ProviderError::Identity(format!("could not copy the NVRAM: {error}")))?;
    if non_empty_file(&source.basesystem()) {
        fs::copy(source.basesystem(), template.basesystem()).map_err(|error| {
            ProviderError::Identity(format!("could not copy the install media: {error}"))
        })?;
    }
    if !template.ready() {
        return Err(ProviderError::Identity(
            "the template files were not all written".to_string(),
        ));
    }
    report(TemplateSavePhase::Completed, "Template saved", Some(100));

    Ok(template.size_bytes())
}

/// Gives a new machine its disk from a template: the overlay through the image's own
/// `qemu-img`, then the NVRAM and install media copied so the clone boots the way the
/// template did. Runs when the machine's disk is first prepared.
pub(crate) fn clone_template_files(
    template_dir: &Path,
    disk: &MachineDisk,
) -> Result<(), ProviderError> {
    let template = MachineTemplateFiles::new(template_dir)?;
    if !template.ready() {
        return Err(ProviderError::Identity(format!(
            "the template at {} is incomplete; save it again or create the machine from a fresh install",
            template_dir.display()
        )));
    }
    run_docker(
        "template clone",
        &template_clone_args(template_dir, disk.dir()),
    )?;
    fs::copy(template.nvram(), disk.nvram())
        .map_err(|error| ProviderError::Identity(format!("could not copy the NVRAM: {error}")))?;
    if non_empty_file(&template.basesystem()) {
        fs::copy(template.basesystem(), disk.basesystem()).map_err(|error| {
            ProviderError::Identity(format!("could not copy the install media: {error}"))
        })?;
    }

    Ok(())
}

/// Gives a clone its own access key through the template's: the machine's public key is
/// appended to the guest's authorized keys and the template's is retired from them, over one
/// SSH session with the template's private key against the pinned host key. The caller then
/// proves the new key works before trusting the result.
pub fn adopt_guest_key(
    ssh_port: u16,
    username: &str,
    template_identity_path: &Path,
    known_hosts_path: &Path,
    public_key: &str,
    retire_public_key: &str,
) -> Result<(), ProviderError> {
    if !crate::valid_guest_username(username) {
        return Err(ProviderError::GuestBridge(
            "the template's guest username is invalid".to_string(),
        ));
    }
    if !crate::valid_guest_public_key(public_key)
        || !crate::valid_guest_public_key(retire_public_key)
    {
        return Err(ProviderError::GuestBridge(
            "a guest access key is not a single-line OpenSSH public key".to_string(),
        ));
    }
    let script = adopt_key_script(public_key, retire_public_key);
    crate::ssh::run_guest_command(
        ssh_port,
        username,
        template_identity_path,
        known_hosts_path,
        &script,
    )
    .map(|_| ())
}

/// Appends the new key once, drops the retired one, and restores the permissions sshd insists
/// on. Both keys are single-quoted whole; `-x` matches whole lines so a prefix never matches.
pub(crate) fn adopt_key_script(public_key: &str, retire_public_key: &str) -> String {
    let new = crate::ssh::shell_single_quote(public_key);
    let old = crate::ssh::shell_single_quote(retire_public_key);
    format!(
        "set -e; umask 077; /bin/mkdir -p ~/.ssh; /usr/bin/touch ~/.ssh/authorized_keys; \
         /usr/bin/grep -qxF {new} ~/.ssh/authorized_keys || /usr/bin/printf '%s\\n' {new} >> ~/.ssh/authorized_keys; \
         /usr/bin/grep -vxF {old} ~/.ssh/authorized_keys > ~/.ssh/authorized_keys.buildbridge || /usr/bin/true; \
         /bin/mv ~/.ssh/authorized_keys.buildbridge ~/.ssh/authorized_keys; \
         /bin/chmod 700 ~/.ssh; /bin/chmod 600 ~/.ssh/authorized_keys"
    )
}

/// Deletes a template's directory. Callers refuse first while any machine's disk is an overlay
/// over it; that overlay would be unreadable without it.
pub fn remove_template_files(template: &MachineTemplateFiles) -> Result<(), ProviderError> {
    match fs::remove_dir_all(template.dir()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ProviderError::Identity(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_template_is_compressed_by_the_images_own_tool_from_a_read_only_source() {
        let source = MachineDisk::new(Path::new("/home/m/machines/one/disk")).unwrap();
        let args = template_compress_args(&source, Path::new("/home/m/templates/xcode"));
        assert_eq!(args[0], "run");
        assert!(
            args.contains(&"--volume=/home/m/machines/one/disk:/buildbridge-source:ro".to_string())
        );
        assert!(
            args.contains(&"--volume=/home/m/templates/xcode:/buildbridge-output:rw".to_string())
        );
        assert!(!args.iter().any(|arg| arg.contains("/buildbridge-template")));
        assert!(args.ends_with(&[
            "-O".to_string(),
            "qcow2".to_string(),
            "/buildbridge-source/mac_hdd_ng.img".to_string(),
            "/buildbridge-output/mac_hdd_ng.img.part".to_string(),
        ]));
        assert!(args.contains(&"-c".to_string()), "compressed");
        assert!(args.contains(&"-p".to_string()), "with progress");

        // A clone's disk reads through its template, which the conversion flattens away.
        let clone = MachineDisk::from_template(
            Path::new("/home/m/machines/two/disk"),
            Path::new("/home/m/templates/base"),
        )
        .unwrap();
        let args = template_compress_args(&clone, Path::new("/home/m/templates/xcode"));
        assert!(
            args.contains(&"--volume=/home/m/templates/base:/buildbridge-template:ro".to_string())
        );
    }

    #[test]
    fn a_clone_is_an_overlay_whose_backing_path_is_where_the_container_binds_the_template() {
        let args = template_clone_args(
            Path::new("/home/m/templates/xcode"),
            Path::new("/home/m/machines/two/disk"),
        );
        assert!(
            args.contains(&"--volume=/home/m/templates/xcode:/buildbridge-template:ro".to_string())
        );
        assert!(
            args.contains(&"--volume=/home/m/machines/two/disk:/buildbridge-disk:rw".to_string())
        );
        let backing = args
            .iter()
            .position(|arg| arg == "-b")
            .expect("a backing file");
        assert_eq!(args[backing + 1], "/buildbridge-template/mac_hdd_ng.img");
        assert_eq!(args[args.len() - 1], "/buildbridge-disk/mac_hdd_ng.img");
    }

    #[test]
    fn convert_progress_is_read_from_the_last_report() {
        assert_eq!(
            parse_convert_progress("    (0.00/100%)\r    (12.34/100%)"),
            Some(12)
        );
        assert_eq!(parse_convert_progress("    (100.00/100%)\r"), Some(100));
        assert_eq!(parse_convert_progress("nothing yet"), None);
    }

    #[test]
    fn the_adopt_script_appends_the_new_key_once_and_retires_the_old_one_whole() {
        let script = adopt_key_script("ssh-ed25519 AAAAnew host", "ssh-ed25519 AAAAold host");
        assert!(script.contains("grep -qxF 'ssh-ed25519 AAAAnew host' ~/.ssh/authorized_keys ||"));
        assert!(script.contains("grep -vxF 'ssh-ed25519 AAAAold host' ~/.ssh/authorized_keys >"));
        assert!(script.contains("chmod 600 ~/.ssh/authorized_keys"));
        assert!(script.starts_with("set -e; umask 077;"));
    }

    #[test]
    fn template_files_are_ready_only_with_an_image_and_nvram() {
        let template = MachineTemplateFiles::new(Path::new("/nonexistent/template")).expect("path");
        assert!(!template.ready());
        assert_eq!(template.size_bytes(), 0);
        assert!(MachineTemplateFiles::new(Path::new("relative")).is_err());
    }
}
