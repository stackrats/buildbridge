//! Host-side USB: which iPhones are plugged in, whether the host will let one go, the udev
//! rule that makes it let go, and plugging one into the guest over QMP.
//!
//! Everything is fixed argv. The one privileged step installs a rule file through `pkexec`
//! with a path and a mode; no shell ever sees a value from this module.

use std::fs::{self, OpenOptions};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::disk::inspect_container_layout;
use crate::qmp::{IPHONE_QMP_DEVICE_ID, QmpClient, UsbAttachment, usb_attachment};
use crate::{ContainerState, ProviderError, TrackedCommand, clean_output, recent_logs};
use ts_rs::TS;

/// Sorted after `39-usbmuxd.rules`, whose ownership and systemd activation it overrides, and
/// before the `80-`/`99-` rules that do not touch phones.
pub const USB_UDEV_RULE_PATH: &str = "/etc/udev/rules.d/40-buildbridge-iphone.rules";
/// The rule. It stops usbmuxd from being started for the phone and hands the node to `plugdev`,
/// the group the container's QEMU user is added to. That is deliberately all it does.
///
/// It does **not** choose a USB configuration; the phone is left parked in configuration 0 by
/// `39-usbmuxd.rules`. Choosing one here was tried and is wrong in both directions. An iPhone
/// lists its configurations in increasing capability — on iOS 18: PTP, iPod audio, PTP + Apple
/// Mobile Device, that plus Apple USB Ethernet, and finally that plus NCM. Selecting the first
/// gets a camera; selecting the last is worse, because its network interfaces then appear on
/// this host, Linux binds `cdc_ncm` to them, and the interfaces the guest needs are taken by
/// the wrong machine. Configuration 0 exposes no interfaces at all, so nothing here can bind,
/// and macOS chooses a configuration itself while enumerating, the way it would over a cable.
pub const USB_UDEV_RULE: &str = "# Written by BuildBridge; remove this file to restore usbmuxd handling of iPhones.\n\
SUBSYSTEM==\"usb\", ENV{DEVTYPE}==\"usb_device\", ENV{PRODUCT}==\"5ac/12[9a][0-9a-f]/*\", ENV{USBMUX_SUPPORTED}=\"0\", ENV{SYSTEMD_WANTS}=\"\", TAG-=\"systemd\", GROUP=\"plugdev\", MODE=\"0660\"\n";
/// The character-device major of `/dev/bus/usb`, for the container's device cgroup rule.
pub const USB_BUS_MAJOR: u32 = 189;
pub const APPLE_VENDOR_ID: &str = "05ac";
pub const PLUGDEV_GROUP: &str = "plugdev";
const SYSFS_USB_DEVICES: &str = "/sys/bus/usb/devices";
const SYSFS_USBFS_DRIVER: &str = "/sys/bus/usb/drivers/usbfs";
const USB_DEVICE_NODES: &str = "/dev/bus/usb";
const PKEXEC: &str = "/usr/bin/pkexec";
const ATTACH_TIMEOUT: Duration = Duration::from_secs(15);
const ATTACH_POLL: Duration = Duration::from_millis(500);

/// Who has the phone's node open on the host side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum UsbHolder {
    Usbmuxd,
    ThisMachine,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct HostUsbDevice {
    pub bus: u8,
    /// The sysfs `devpath`, which is also what QEMU calls `hostport`.
    pub port: String,
    pub vendor_id: String,
    pub product_id: String,
    pub product: Option<String>,
    pub serial: Option<String>,
    pub manufacturer: Option<String>,
    pub device_node: String,
    /// This user can open the node read-write, which is exactly what the container's QEMU
    /// user (the same uid, plus the plugdev group) needs.
    pub node_ready: bool,
    pub held_by: Option<UsbHolder>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum UdevRuleState {
    Missing,
    Installed,
    Modified,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct HostUsbStatus {
    pub supported: bool,
    pub rule: UdevRuleState,
    pub rule_path: String,
    pub usbmuxd_active: bool,
    pub plugdev_gid: Option<u32>,
    pub devices: Vec<HostUsbDevice>,
    pub issues: Vec<String>,
}

/// What a container needs at creation to reach USB devices without `--privileged`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ContainerUsbOptions {
    pub plugdev_gid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AttachedUsbDevice {
    pub bus: u8,
    pub port: String,
    /// The guest has enumerated the phone; until then QEMU holds only the port.
    pub enumerated: bool,
    pub issue: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct MachineUsbStatus {
    pub host: HostUsbStatus,
    pub disk_on_host: bool,
    pub container_ready: bool,
    pub container_issue: Option<String>,
    pub qmp_reachable: bool,
    pub attached: Option<AttachedUsbDevice>,
    /// The container carries the dedicated USB 2.0 controller phones are attached to. Containers
    /// created before it existed need one rebuild; the disk is kept.
    pub phone_controller: bool,
}

/// Apple's mobile devices in normal mode: vendor `05ac`, products `1290`–`12af`. Recovery
/// mode (`8600`) is deliberately excluded; a phone in recovery is not a build target.
pub fn is_apple_mobile_product(vendor_id: &str, product_id: &str) -> bool {
    let vendor = vendor_id.trim().to_ascii_lowercase();
    let product = product_id.trim().to_ascii_lowercase();
    let bytes = product.as_bytes();

    vendor == APPLE_VENDOR_ID
        && bytes.len() == 4
        && bytes[0] == b'1'
        && bytes[1] == b'2'
        && matches!(bytes[2], b'9' | b'a')
        && bytes[3].is_ascii_hexdigit()
}

/// A sysfs `devpath`: port numbers joined by dots, as QEMU builds `hostport` from libusb.
/// This is the one host-derived value that reaches QEMU, so it is checked again at attach.
pub fn valid_usb_port_path(port: &str) -> bool {
    !port.is_empty()
        && port.len() <= 32
        && port.split('.').all(|segment| {
            !segment.is_empty()
                && segment.len() <= 3
                && !segment.starts_with('0')
                && segment.bytes().all(|byte| byte.is_ascii_digit())
        })
}

/// One `/sys/bus/usb/devices/<name>` entry as a device, given a reader for its attribute
/// files. Root hubs (`usbN`) and interfaces (`1-2:1.0`) are not devices.
pub(crate) fn parse_sysfs_device(
    name: &str,
    read: &dyn Fn(&str) -> Option<String>,
) -> Option<HostUsbDevice> {
    if name.starts_with("usb") || name.contains(':') {
        return None;
    }
    let vendor_id = read("idVendor")?.to_ascii_lowercase();
    let product_id = read("idProduct")?.to_ascii_lowercase();
    let bus: u8 = read("busnum")?.parse().ok()?;
    let devnum: u16 = read("devnum")?.parse().ok()?;
    let port = read("devpath")?;
    if !valid_usb_port_path(&port) {
        return None;
    }
    let optional = |attribute: &str| read(attribute).filter(|value| !value.is_empty());

    Some(HostUsbDevice {
        bus,
        port,
        vendor_id,
        product_id,
        product: optional("product"),
        serial: optional("serial"),
        manufacturer: optional("manufacturer"),
        device_node: format!("{USB_DEVICE_NODES}/{bus:03}/{devnum:03}"),
        node_ready: false,
        held_by: None,
    })
}

pub(crate) fn parse_group_id(etc_group: &str, group: &str) -> Option<u32> {
    etc_group.lines().find_map(|line| {
        let mut fields = line.split(':');
        if fields.next()? != group {
            return None;
        }

        fields.nth(1)?.parse().ok()
    })
}

pub(crate) fn rule_state(existing: Option<&str>) -> UdevRuleState {
    match existing {
        None => UdevRuleState::Missing,
        Some(content) if content.trim() == USB_UDEV_RULE.trim() => UdevRuleState::Installed,
        Some(_) => UdevRuleState::Modified,
    }
}

/// `install` writes to a temporary name and renames, so the destination is atomic and never
/// half-written even though the rule directory is read by udevd on every change.
pub(crate) fn install_rule_args(staged: &Path) -> Vec<String> {
    vec![
        "/usr/bin/install".to_string(),
        "-m".to_string(),
        "0644".to_string(),
        "-o".to_string(),
        "root".to_string(),
        "-g".to_string(),
        "root".to_string(),
        staged.display().to_string(),
        USB_UDEV_RULE_PATH.to_string(),
    ]
}

pub(crate) fn remove_rule_args() -> Vec<String> {
    vec![
        "/usr/bin/rm".to_string(),
        "-f".to_string(),
        USB_UDEV_RULE_PATH.to_string(),
    ]
}

pub(crate) fn pkexec_failure(status: ExitStatus, stderr: &str) -> ProviderError {
    ProviderError::HostAuthorization(match status.code() {
        Some(126) => "the authorization prompt was dismissed; the host was not changed".to_string(),
        Some(127) => "authorization failed or no authentication agent is running".to_string(),
        _ if !stderr.trim().is_empty() => stderr.trim().to_string(),
        Some(code) => format!("the privileged command exited with status {code}"),
        None => "the privileged command was terminated".to_string(),
    })
}

/// USB access the host can grant a container, if it has the group and the device tree.
pub fn resolve_usb_options() -> Option<ContainerUsbOptions> {
    if !Path::new(USB_DEVICE_NODES).is_dir() {
        return None;
    }
    let groups = fs::read_to_string("/etc/group").ok()?;

    parse_group_id(&groups, PLUGDEV_GROUP).map(|plugdev_gid| ContainerUsbOptions { plugdev_gid })
}

/// Every Apple mobile device on the host, with what stands between it and the guest.
/// `attached` names the port this machine already holds so its own claim is not mistaken for
/// another program's.
pub fn host_usb_status(attached: Option<(u8, &str)>) -> HostUsbStatus {
    let supported = cfg!(target_os = "linux") && Path::new(SYSFS_USB_DEVICES).is_dir();
    let rule = rule_state(fs::read_to_string(USB_UDEV_RULE_PATH).ok().as_deref());
    let usbmuxd_active = usbmuxd_is_active();
    let plugdev_gid = fs::read_to_string("/etc/group")
        .ok()
        .and_then(|groups| parse_group_id(&groups, PLUGDEV_GROUP));
    let claims = usbfs_claims();
    let mut devices = Vec::new();

    if supported && let Ok(entries) = fs::read_dir(SYSFS_USB_DEVICES) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let directory = entry.path();
            let read = |attribute: &str| {
                fs::read_to_string(directory.join(attribute))
                    .ok()
                    .map(|value| value.trim().to_string())
            };
            let Some(mut device) = parse_sysfs_device(&name, &read) else {
                continue;
            };
            if !is_apple_mobile_product(&device.vendor_id, &device.product_id) {
                continue;
            }
            device.node_ready = node_accessible(Path::new(&device.device_node));
            device.held_by = holder_for(&device, &claims, attached, usbmuxd_active);
            devices.push(device);
        }
    }
    devices.sort_by(|left, right| (left.bus, &left.port).cmp(&(right.bus, &right.port)));

    let mut issues = Vec::new();
    if !supported {
        issues.push("USB passthrough needs a Linux host with sysfs.".to_string());
    }
    match rule {
        UdevRuleState::Missing => issues.push(
            "Install the BuildBridge iPhone rule so usbmuxd releases phones to the machine."
                .to_string(),
        ),
        UdevRuleState::Modified => issues.push(
            "The BuildBridge iPhone rule on this host differs from the expected content; reinstall it."
                .to_string(),
        ),
        UdevRuleState::Installed => {}
    }
    if plugdev_gid.is_none() {
        issues.push(
            "This host has no plugdev group, so the container cannot be granted USB access."
                .to_string(),
        );
    }
    for device in &devices {
        let label = device.product.as_deref().unwrap_or("the phone");
        if device.held_by == Some(UsbHolder::Usbmuxd) {
            issues.push(format!(
                "usbmuxd is holding {label}; unplug it and plug it in again."
            ));
        } else if !device.node_ready && rule == UdevRuleState::Installed {
            issues.push(format!(
                "Unplug {label} and plug it in again so the rule applies to it."
            ));
        }
    }

    HostUsbStatus {
        supported,
        rule,
        rule_path: USB_UDEV_RULE_PATH.to_string(),
        usbmuxd_active,
        plugdev_gid,
        devices,
        issues,
    }
}

fn holder_for(
    device: &HostUsbDevice,
    claims: &[String],
    attached: Option<(u8, &str)>,
    usbmuxd_active: bool,
) -> Option<UsbHolder> {
    let prefix = format!("{}-{}:", device.bus, device.port);
    if !claims.iter().any(|claim| claim.starts_with(&prefix)) {
        return None;
    }
    if attached.is_some_and(|(bus, port)| bus == device.bus && port == device.port) {
        Some(UsbHolder::ThisMachine)
    } else if usbmuxd_active {
        Some(UsbHolder::Usbmuxd)
    } else {
        Some(UsbHolder::Other)
    }
}

/// Interfaces claimed through usbfs by any process: `3-2.3.1:4.1` for the phone on bus 3
/// port 2.3.1. usbmuxd and the container's QEMU both show up here.
fn usbfs_claims() -> Vec<String> {
    fs::read_dir(SYSFS_USBFS_DRIVER)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().to_string())
                .filter(|name| name.contains('-'))
                .collect()
        })
        .unwrap_or_default()
}

fn node_accessible(node: &Path) -> bool {
    OpenOptions::new().read(true).write(true).open(node).is_ok()
}

fn usbmuxd_is_active() -> bool {
    Command::new("systemctl")
        .args(["is-active", "usbmuxd.service"])
        .output()
        .is_ok_and(|output| clean_output(&output.stdout) == "active")
}

fn ensure_pkexec() -> Result<(), ProviderError> {
    if !cfg!(target_os = "linux") || !Path::new(PKEXEC).is_file() {
        return Err(ProviderError::Prerequisites(
            "installing the USB rule needs pkexec (polkit) on a Linux host".to_string(),
        ));
    }

    Ok(())
}

fn run_pkexec(args: &[String]) -> Result<(), ProviderError> {
    let output = Command::new(PKEXEC)
        .args(args)
        .tracked_output()
        .map_err(|error| ProviderError::HostAuthorization(error.to_string()))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(pkexec_failure(output.status, &clean_output(&output.stderr)))
    }
}

/// Installs the rule through one authorization prompt. udevd reloads rules on its own when
/// the directory changes; the phone has to be plugged in again for the new ownership to apply,
/// which the caller's status makes visible.
pub fn install_iphone_udev_rule(staging_dir: &Path) -> Result<HostUsbStatus, ProviderError> {
    ensure_pkexec()?;
    fs::create_dir_all(staging_dir)
        .map_err(|error| ProviderError::HostAuthorization(error.to_string()))?;
    let staged = staging_dir.join("40-buildbridge-iphone.rules");
    fs::write(&staged, USB_UDEV_RULE)
        .and_then(|()| fs::set_permissions(&staged, fs::Permissions::from_mode(0o644)))
        .map_err(|error| ProviderError::HostAuthorization(error.to_string()))?;
    let installed = run_pkexec(&install_rule_args(&staged));
    let _ = fs::remove_file(&staged);
    installed?;

    Ok(host_usb_status(None))
}

pub fn remove_iphone_udev_rule() -> Result<HostUsbStatus, ProviderError> {
    ensure_pkexec()?;
    run_pkexec(&remove_rule_args())?;

    Ok(host_usb_status(None))
}

/// Hands one host port to the guest. `device_add` returns before QEMU opens the device, and an
/// open failure only reaches QEMU's own stderr, so the call waits for the guest to enumerate
/// the phone and otherwise reports the last USB line the container logged.
pub fn attach_usb_device(
    qmp_socket: &Path,
    container_name: &str,
    device: &HostUsbDevice,
    guest_reset: bool,
) -> Result<AttachedUsbDevice, ProviderError> {
    if !valid_usb_port_path(&device.port)
        || !is_apple_mobile_product(&device.vendor_id, &device.product_id)
    {
        return Err(ProviderError::UsbPassthrough(
            "the device is not an Apple mobile device on a valid port".to_string(),
        ));
    }
    if !node_accessible(Path::new(&device.device_node)) {
        return Err(ProviderError::UsbPassthrough(
            "this user cannot open the phone's device node; install the USB rule, then unplug the phone and plug it in again".to_string(),
        ));
    }

    let mut client = QmpClient::connect(qmp_socket)?;
    // QEMU reads a phone cleanly only the first time it opens it in a process, so a phone it
    // already holds is never released and re-added here: one it can read is left exactly as it
    // is, and one it could not read needs a replug, which only a person can do.
    if client
        .peripheral_ids()?
        .iter()
        .any(|id| id == IPHONE_QMP_DEVICE_ID)
    {
        let held = match client.usb_summary()? {
            Some(text) => usb_attachment(&text, IPHONE_QMP_DEVICE_ID),
            None => UsbAttachment::Absent,
        };
        match held {
            UsbAttachment::Live => {
                return Ok(AttachedUsbDevice {
                    bus: device.bus,
                    port: device.port.clone(),
                    enumerated: true,
                    issue: None,
                });
            }
            UsbAttachment::Unreadable => {
                return Err(ProviderError::UsbPassthrough(
                    "QEMU holds this phone but could not read it, which happens once a phone has been detached and attached again in the same session, or has re-enumerated under QEMU. Detach it, unplug it, plug it in again, and attach once; if that repeats, restart the machine."
                        .to_string(),
                ));
            }
            UsbAttachment::Absent => client.delete_device(IPHONE_QMP_DEVICE_ID)?,
        }
    }
    client.add_usb_host(device.bus, &device.port, guest_reset)?;

    let started = Instant::now();
    let mut attachment = UsbAttachment::Absent;
    while started.elapsed() < ATTACH_TIMEOUT {
        attachment = match client.usb_summary()? {
            Some(text) => usb_attachment(&text, IPHONE_QMP_DEVICE_ID),
            None => UsbAttachment::Absent,
        };
        if attachment == UsbAttachment::Live {
            break;
        }
        thread::sleep(ATTACH_POLL);
    }
    let enumerated = attachment == UsbAttachment::Live;
    // A phone QEMU holds but cannot read is its own state: no log line explains it, and only
    // a physical replug clears it, so say that rather than showing an unrelated libusb line.
    let issue = match attachment {
        UsbAttachment::Live => None,
        UsbAttachment::Unreadable => Some(
            "The phone was reset while it was being handed over, so macOS cannot enumerate it. Unplug it, plug it in again, and attach once."
                .to_string(),
        ),
        UsbAttachment::Absent => Some(
            recent_logs(container_name)
                .ok()
                .and_then(|lines| {
                    lines.into_iter().rev().find(|line| {
                        let lowered = line.to_ascii_lowercase();
                        lowered.contains("usb-host") || lowered.contains("libusb")
                    })
                })
                .unwrap_or_else(|| {
                    "The guest has not enumerated the phone yet; unplug it and plug it in again."
                        .to_string()
                }),
        ),
    };

    Ok(AttachedUsbDevice {
        bus: device.bus,
        port: device.port.clone(),
        enumerated,
        issue,
    })
}

pub fn detach_usb_device(qmp_socket: &Path) -> Result<(), ProviderError> {
    QmpClient::connect(qmp_socket)?.delete_device(IPHONE_QMP_DEVICE_ID)
}

/// What the guest currently holds, or `None` when QEMU is unreachable or holds nothing.
pub fn attached_usb_device(qmp_socket: &Path) -> Option<AttachedUsbDevice> {
    probe_qmp(qmp_socket).1
}

fn probe_qmp(qmp_socket: &Path) -> (bool, Option<AttachedUsbDevice>) {
    let Ok(mut client) = QmpClient::connect(qmp_socket) else {
        return (false, None);
    };
    let held = client
        .peripheral_ids()
        .is_ok_and(|ids| ids.iter().any(|id| id == IPHONE_QMP_DEVICE_ID));
    if !held {
        return (true, None);
    }
    let bus = client
        .device_property(IPHONE_QMP_DEVICE_ID, "hostbus")
        .ok()
        .flatten()
        .and_then(|value| value.as_u64())
        .and_then(|value| u8::try_from(value).ok());
    let port = client
        .device_property(IPHONE_QMP_DEVICE_ID, "hostport")
        .ok()
        .flatten()
        .and_then(|value| value.as_str().map(str::to_string))
        .filter(|port| valid_usb_port_path(port));
    let (Some(bus), Some(port)) = (bus, port) else {
        return (true, None);
    };
    let attachment = client
        .usb_summary()
        .ok()
        .flatten()
        .map(|text| usb_attachment(&text, IPHONE_QMP_DEVICE_ID))
        .unwrap_or(UsbAttachment::Absent);

    (
        true,
        Some(AttachedUsbDevice {
            bus,
            port,
            enumerated: attachment == UsbAttachment::Live,
            issue: (attachment == UsbAttachment::Unreadable).then(|| {
                "The phone was reset while it was being handed over, so macOS cannot enumerate it. Unplug it, plug it in again, and attach once."
                    .to_string()
            }),
        }),
    )
}

/// The whole USB picture for one machine: host, container, control socket, and attachment.
pub fn machine_usb_status(
    container_name: &str,
    qmp_socket: &Path,
    state: ContainerState,
) -> MachineUsbStatus {
    let running = state == ContainerState::Running;
    let (qmp_reachable, attached) = if running {
        probe_qmp(qmp_socket)
    } else {
        (false, None)
    };
    let host = host_usb_status(
        attached
            .as_ref()
            .map(|attached| (attached.bus, attached.port.as_str())),
    );
    let layout = match state {
        ContainerState::Missing | ContainerState::Unavailable => None,
        _ => inspect_container_layout(container_name).ok().flatten(),
    };
    let disk_on_host = layout.as_ref().is_some_and(|layout| layout.disk_on_host);
    let phone_controller = layout
        .as_ref()
        .is_some_and(|layout| layout.phone_controller);
    let container_ready = layout.as_ref().is_some_and(|layout| {
        layout.disk_on_host && layout.usb_access && layout.control_socket && layout.phone_controller
    });
    let container_issue = match (state, layout.as_ref()) {
        (ContainerState::Unavailable, _) => {
            Some("Docker is unavailable, so the container cannot be inspected.".to_string())
        }
        (ContainerState::Missing, _) => Some(
            "The container gets USB access and a host-side disk when the machine is next started."
                .to_string(),
        ),
        (_, Some(layout)) if layout.disk_on_host && layout.usb_access && !layout.phone_controller => {
            Some(
                "Rebuild this machine's container once to add the phone's USB controller; the macOS disk is kept."
                    .to_string(),
            )
        }
        (_, Some(_)) if !container_ready => Some(
            "Enable USB on this machine to recreate its container with USB access; the macOS disk is kept."
                .to_string(),
        ),
        (_, None) => Some("The container could not be inspected.".to_string()),
        _ => None,
    };

    MachineUsbStatus {
        host,
        disk_on_host,
        container_ready,
        container_issue,
        qmp_reachable,
        attached,
        phone_controller,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::os::unix::process::ExitStatusExt;

    use super::*;

    #[test]
    fn apple_mobile_devices_are_recognized_by_vendor_and_product_id() {
        for (vendor, product) in [("05ac", "12a8"), ("05ac", "1290"), ("05AC", "12A8")] {
            assert!(
                is_apple_mobile_product(vendor, product),
                "{vendor}:{product}"
            );
        }
        for (vendor, product) in [
            ("05ac", "8600"),
            ("046d", "c542"),
            ("1d6b", "0002"),
            ("05ac", "12"),
            ("05ac", "12g0"),
        ] {
            assert!(
                !is_apple_mobile_product(vendor, product),
                "{vendor}:{product}"
            );
        }
    }

    #[test]
    fn usb_port_paths_are_validated_before_reaching_qemu() {
        for port in ["1", "1.2", "10.3.4", "2.3.1"] {
            assert!(valid_usb_port_path(port), "{port}");
        }
        for port in [
            "",
            "0",
            "1.",
            ".1",
            "1..2",
            "1-2",
            "a",
            "01",
            "1.0",
            "1234",
            "1;2",
            "1.2.3.4.5.6.7.8.9.10.11.12.13.14.15",
        ] {
            assert!(!valid_usb_port_path(port), "{port}");
        }
    }

    fn reader(values: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = values
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        move |attribute: &str| map.get(attribute).cloned()
    }

    #[test]
    fn sysfs_entries_map_to_host_devices_and_skip_hubs_and_interfaces() {
        let read = reader(&[
            ("idVendor", "05ac"),
            ("idProduct", "12a8"),
            ("busnum", "3"),
            ("devnum", "7"),
            ("devpath", "2.3.1"),
            ("product", "iPhone"),
            ("serial", "00008030000A1B2C3D4E5F6A"),
            ("manufacturer", "Apple Inc."),
        ]);
        let device = parse_sysfs_device("3-2.3.1", &read).expect("a device");
        assert_eq!(device.bus, 3);
        assert_eq!(device.port, "2.3.1");
        assert_eq!(device.device_node, "/dev/bus/usb/003/007");
        assert_eq!(device.product.as_deref(), Some("iPhone"));
        assert!(!device.node_ready);

        assert!(parse_sysfs_device("usb3", &read).is_none());
        assert!(parse_sysfs_device("3-2.3.1:1.0", &read).is_none());
        let unreadable = reader(&[("idVendor", "05ac")]);
        assert!(parse_sysfs_device("3-2", &unreadable).is_none());
        let bad_port = reader(&[
            ("idVendor", "05ac"),
            ("idProduct", "12a8"),
            ("busnum", "3"),
            ("devnum", "7"),
            ("devpath", "2..3"),
        ]);
        assert!(parse_sysfs_device("3-2..3", &bad_port).is_none());
    }

    #[test]
    fn the_udev_rule_is_installed_with_fixed_argv_and_never_a_shell() {
        let args = install_rule_args(Path::new(
            "/tmp/buildbridge/usb/40-buildbridge-iphone.rules",
        ));
        assert_eq!(
            args,
            vec![
                "/usr/bin/install",
                "-m",
                "0644",
                "-o",
                "root",
                "-g",
                "root",
                "/tmp/buildbridge/usb/40-buildbridge-iphone.rules",
                "/etc/udev/rules.d/40-buildbridge-iphone.rules",
            ]
        );
        assert_eq!(
            remove_rule_args(),
            vec![
                "/usr/bin/rm",
                "-f",
                "/etc/udev/rules.d/40-buildbridge-iphone.rules"
            ]
        );
        for arg in args.iter().chain(remove_rule_args().iter()) {
            assert!(!arg.contains(';') && !arg.contains("$(") && !arg.contains(' '));
        }
    }

    #[test]
    fn the_udev_rule_content_disables_usbmuxd_and_grants_plugdev() {
        let rules: Vec<&str> = USB_UDEV_RULE
            .lines()
            .filter(|line| !line.starts_with('#'))
            .collect();
        assert_eq!(rules.len(), 1);
        let rule = rules[0];
        for fragment in [
            "ENV{PRODUCT}==\"5ac/12[9a][0-9a-f]/*\"",
            "ENV{USBMUX_SUPPORTED}=\"0\"",
            "ENV{SYSTEMD_WANTS}=\"\"",
            "TAG-=\"systemd\"",
            "GROUP=\"plugdev\"",
            "MODE=\"0660\"",
        ] {
            assert!(rule.contains(fragment), "{fragment}");
        }
        // Choosing a configuration on this host is what hands the phone's network interfaces
        // to Linux's own drivers; the guest has to be the one that chooses.
        assert!(
            !rule.contains("bConfigurationValue"),
            "the phone stays unconfigured here so that no driver on this host binds to it"
        );
        assert!(USB_UDEV_RULE.ends_with('\n'));
    }

    #[test]
    fn rule_state_distinguishes_missing_installed_and_modified_content() {
        assert_eq!(rule_state(None), UdevRuleState::Missing);
        assert_eq!(rule_state(Some(USB_UDEV_RULE)), UdevRuleState::Installed);
        assert_eq!(
            rule_state(Some(&format!("{USB_UDEV_RULE}\n\n"))),
            UdevRuleState::Installed
        );
        assert_eq!(
            rule_state(Some("SUBSYSTEM==\"usb\", MODE=\"0666\"\n")),
            UdevRuleState::Modified
        );
    }

    #[test]
    fn plugdev_gid_is_read_from_the_group_database() {
        let groups = "root:x:0:\nplugdev:x:46:matt\ndocker:x:987:matt\n";
        assert_eq!(parse_group_id(groups, "plugdev"), Some(46));
        assert_eq!(parse_group_id(groups, "docker"), Some(987));
        assert_eq!(parse_group_id(groups, "usbmux"), None);
        assert_eq!(parse_group_id("plugdev:x:abc:\n", "plugdev"), None);
    }

    #[test]
    fn pkexec_exit_codes_map_to_user_facing_messages() {
        let dismissed = pkexec_failure(ExitStatus::from_raw(126 << 8), "");
        assert!(dismissed.to_string().contains("dismissed"));
        let no_agent = pkexec_failure(ExitStatus::from_raw(127 << 8), "");
        assert!(no_agent.to_string().contains("agent"));
        let other = pkexec_failure(
            ExitStatus::from_raw(1 << 8),
            "install: cannot create regular file",
        );
        assert!(other.to_string().contains("cannot create"));
    }

    #[test]
    fn holders_are_attributed_by_claim_and_attachment() {
        let read = reader(&[
            ("idVendor", "05ac"),
            ("idProduct", "12a8"),
            ("busnum", "3"),
            ("devnum", "7"),
            ("devpath", "2"),
        ]);
        let device = parse_sysfs_device("3-2", &read).expect("a device");
        let claims = vec!["3-2:4.1".to_string(), "1-4:1.0".to_string()];
        assert_eq!(
            holder_for(&device, &claims, None, true),
            Some(UsbHolder::Usbmuxd)
        );
        assert_eq!(
            holder_for(&device, &claims, None, false),
            Some(UsbHolder::Other)
        );
        assert_eq!(
            holder_for(&device, &claims, Some((3, "2")), true),
            Some(UsbHolder::ThisMachine)
        );
        assert_eq!(holder_for(&device, &[], None, true), None);
    }
}
