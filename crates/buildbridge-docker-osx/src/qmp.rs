//! A small QMP client for the machine's QEMU: JSON lines over the unix socket QEMU exposes
//! through the bind-mounted control directory.
//!
//! It exists to plug a host USB device into the guest and to ask what the guest sees. It never
//! touches the disk, the network, or the monitor's human-readable commands beyond the one
//! `x-query-usb` summary, and every request is built from validated values.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::{MachineProvider, ProviderError};

/// The QEMU device id of the phone. Fixed, so the guest can hold at most one and a re-attach
/// replaces rather than stacks.
pub(crate) const IPHONE_QMP_DEVICE_ID: &str = "buildbridge-iphone";
/// Docker-OSX's `Launch.sh` defines `-device qemu-xhci,id=xhci`; USB 3 devices attach there.
/// The controller a phone attached at boot is given, separate from the machine's own xHCI.
pub(crate) const USB_PHONE_CONTROLLER: &str = "buildbridge-phone-usb";
/// Where the control directory is mounted inside the container.
pub const QMP_CONTAINER_DIR: &str = "/buildbridge-qmp";
/// The socket QEMU creates inside that directory.
pub const QMP_SOCKET_NAME: &str = "qmp.sock";
const QMP_TIMEOUT: Duration = Duration::from_secs(5);
const PERIPHERAL_PATH: &str = "/machine/peripheral";

/// How a machine's QEMU control socket is reached: directly, when QEMU created it in a
/// directory bound from this host, or through `docker exec` and netcat when QEMU runs as root
/// inside the container and its socket is not this user's to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QmpEndpoint {
    Socket(PathBuf),
    ContainerExec { container: String, socket: String },
}

impl QmpEndpoint {
    pub fn for_machine(provider: MachineProvider, qmp_dir: &Path, container_name: &str) -> Self {
        match provider {
            MachineProvider::DockerOsx => Self::Socket(qmp_dir.join(QMP_SOCKET_NAME)),
            MachineProvider::DockurMacos => Self::ContainerExec {
                container: container_name.to_string(),
                socket: crate::dockur::QMP_CONTAINER_SOCKET.to_string(),
            },
        }
    }
}

/// Where the lines come from. Through the relay they arrive by way of a thread, so a socket
/// QEMU is serving to someone else (it serves one client at a time) times out instead of
/// holding the engine.
enum Transport {
    Direct(BufReader<UnixStream>),
    Relay {
        lines: Receiver<std::io::Result<String>>,
        relay: Child,
    },
}

pub(crate) struct QmpClient {
    transport: Transport,
    /// Dropped first on the way out: closing the relay's input is what ends netcat.
    writer: Option<Box<dyn Write + Send>>,
}

/// How long netcat gets to leave after its input closes before the relay is killed.
const RELAY_EXIT_GRACE: Duration = Duration::from_secs(3);
const CLOSED: &str = "QEMU closed the control socket";

impl Drop for QmpClient {
    fn drop(&mut self) {
        // Closing the input ends netcat a second later, and QEMU then serves the next client;
        // killing the `docker exec` on this side would leave netcat holding the socket.
        drop(self.writer.take());
        if let Transport::Relay { relay, .. } = &mut self.transport {
            let deadline = Instant::now() + RELAY_EXIT_GRACE;
            while Instant::now() < deadline {
                if matches!(relay.try_wait(), Ok(Some(_))) {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
            let _ = relay.kill();
            let _ = relay.wait();
        }
    }
}

fn not_running() -> ProviderError {
    ProviderError::UsbPassthrough(
        "the machine is not running, so its QEMU control socket is closed".to_string(),
    )
}

/// QEMU's own description of a refused command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QmpFailure {
    pub class: String,
    pub desc: String,
}

#[derive(Debug)]
pub(crate) enum QmpError {
    Transport(String),
    Command(QmpFailure),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum QmpMessage {
    Greeting,
    Return(Value),
    Error(QmpFailure),
    Event,
    Unknown,
}

impl QmpClient {
    /// Connects, reads the greeting, and negotiates capabilities. A closed socket means the
    /// machine is not running: QEMU removes nothing on exit, so the stale file refuses.
    pub fn connect(endpoint: &QmpEndpoint) -> Result<Self, ProviderError> {
        let (transport, writer): (Transport, Box<dyn Write + Send>) = match endpoint {
            QmpEndpoint::Socket(socket) => {
                let stream = UnixStream::connect(socket).map_err(|error| match error.kind() {
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                        not_running()
                    }
                    _ => ProviderError::UsbPassthrough(format!(
                        "could not open the QEMU control socket: {error}"
                    )),
                })?;
                stream
                    .set_read_timeout(Some(QMP_TIMEOUT))
                    .and_then(|()| stream.set_write_timeout(Some(QMP_TIMEOUT)))
                    .map_err(|error| ProviderError::UsbPassthrough(error.to_string()))?;
                let writer = stream
                    .try_clone()
                    .map_err(|error| ProviderError::UsbPassthrough(error.to_string()))?;
                (Transport::Direct(BufReader::new(stream)), Box::new(writer))
            }
            QmpEndpoint::ContainerExec { container, socket } => {
                // `-q 1`: netcat leaves a second after its input closes, which frees the socket.
                let mut relay = Command::new("docker")
                    .args([
                        "exec",
                        "--interactive",
                        container,
                        "nc",
                        "-q",
                        "1",
                        "-U",
                        socket,
                    ])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|error| {
                        ProviderError::UsbPassthrough(format!(
                            "could not reach the container's QEMU control socket: {error}"
                        ))
                    })?;
                let (Some(stdout), Some(stdin)) = (relay.stdout.take(), relay.stdin.take()) else {
                    return Err(ProviderError::UsbPassthrough(
                        "the control socket relay has no pipes".to_string(),
                    ));
                };
                let (sender, lines) = mpsc::channel();
                thread::spawn(move || {
                    let mut reader = BufReader::new(stdout);
                    loop {
                        let mut line = String::new();
                        let more = match reader.read_line(&mut line) {
                            Ok(0) => {
                                let _ = sender.send(Ok(String::new()));
                                false
                            }
                            Ok(_) => sender.send(Ok(line)).is_ok(),
                            Err(error) => {
                                let _ = sender.send(Err(error));
                                false
                            }
                        };
                        if !more {
                            break;
                        }
                    }
                });
                (Transport::Relay { lines, relay }, Box::new(stdin))
            }
        };
        let mut client = Self {
            transport,
            writer: Some(writer),
        };

        let mut greeted = false;
        for _ in 0..8 {
            match client.read_message() {
                Ok(QmpMessage::Greeting) => {
                    greeted = true;
                    break;
                }
                Ok(QmpMessage::Event) => continue,
                Ok(_) => break,
                // The relay exits at once when nothing listens on the socket: not running.
                Err(QmpError::Transport(message))
                    if message == CLOSED && matches!(client.transport, Transport::Relay { .. }) =>
                {
                    return Err(not_running());
                }
                Err(error) => return Err(transport_error(error)),
            }
        }
        if !greeted {
            return Err(ProviderError::UsbPassthrough(
                "the QEMU control socket did not greet as QMP".to_string(),
            ));
        }
        client
            .execute(&capabilities_request())
            .map_err(|error| map_qmp_failure("handshake", error))?;

        Ok(client)
    }

    fn read_line(&mut self) -> Result<String, QmpError> {
        match &mut self.transport {
            Transport::Direct(reader) => {
                let mut line = String::new();
                let read = reader
                    .read_line(&mut line)
                    .map_err(|error| QmpError::Transport(error.to_string()))?;
                if read == 0 {
                    return Err(QmpError::Transport(CLOSED.to_string()));
                }
                Ok(line)
            }
            Transport::Relay { lines, .. } => match lines.recv_timeout(QMP_TIMEOUT) {
                Ok(Ok(line)) if line.is_empty() => Err(QmpError::Transport(CLOSED.to_string())),
                Ok(Ok(line)) => Ok(line),
                Ok(Err(error)) => Err(QmpError::Transport(error.to_string())),
                Err(RecvTimeoutError::Timeout) => Err(QmpError::Transport(
                    "the QEMU control socket did not answer in time; another client may hold it"
                        .to_string(),
                )),
                Err(RecvTimeoutError::Disconnected) => Err(QmpError::Transport(CLOSED.to_string())),
            },
        }
    }

    fn read_message(&mut self) -> Result<QmpMessage, QmpError> {
        let line = self.read_line()?;
        let value: Value = serde_json::from_str(line.trim())
            .map_err(|error| QmpError::Transport(format!("invalid QMP message: {error}")))?;

        Ok(classify_response(&value))
    }

    /// Sends one command and returns its `return` value, skipping asynchronous events.
    pub fn execute(&mut self, request: &Value) -> Result<Value, QmpError> {
        let mut encoded = serde_json::to_string(request)
            .map_err(|error| QmpError::Transport(error.to_string()))?;
        encoded.push('\n');
        self.writer
            .as_mut()
            .ok_or_else(|| QmpError::Transport(CLOSED.to_string()))?
            .write_all(encoded.as_bytes())
            .map_err(|error| QmpError::Transport(error.to_string()))?;

        loop {
            match self.read_message()? {
                QmpMessage::Return(value) => return Ok(value),
                QmpMessage::Error(failure) => return Err(QmpError::Command(failure)),
                QmpMessage::Event | QmpMessage::Greeting => continue,
                QmpMessage::Unknown => {
                    return Err(QmpError::Transport("unexpected QMP message".to_string()));
                }
            }
        }
    }

    pub fn add_usb_host(
        &mut self,
        bus: u8,
        port: &str,
        guest_reset: bool,
    ) -> Result<(), ProviderError> {
        self.execute(&device_add_request(bus, port, guest_reset))
            .map(|_| ())
            .map_err(|error| map_qmp_failure("device attach", error))
    }

    pub fn add_usb_host_by_node(
        &mut self,
        node: &str,
        guest_reset: bool,
    ) -> Result<(), ProviderError> {
        self.execute(&device_add_by_node_request(node, guest_reset))
            .map(|_| ())
            .map_err(|error| map_qmp_failure("device attach", error))
    }

    /// Removes the device object; a device that is already gone counts as removed.
    pub fn delete_device(&mut self, id: &str) -> Result<(), ProviderError> {
        match self.execute(&device_del_request(id)) {
            Ok(_) => Ok(()),
            Err(QmpError::Command(failure)) if failure.class == "DeviceNotFound" => Ok(()),
            Err(error) => Err(map_qmp_failure("device detach", error)),
        }
    }

    /// Asks the guest to shut itself down, the way pressing a power button would. QEMU exits
    /// once macOS has finished, so the container stopping is the signal that it worked.
    pub fn power_down(&mut self) -> Result<(), ProviderError> {
        self.execute(&json!({"execute": "system_powerdown"}))
            .map(|_| ())
            .map_err(|error| map_qmp_failure("guest shutdown", error))
    }

    pub fn peripheral_ids(&mut self) -> Result<Vec<String>, ProviderError> {
        self.execute(&qom_list_request(PERIPHERAL_PATH))
            .map(|value| parse_peripheral_ids(&value))
            .map_err(|error| map_qmp_failure("device listing", error))
    }

    /// One property of a device object, or `None` when the object or property is absent.
    pub fn device_property(
        &mut self,
        id: &str,
        property: &str,
    ) -> Result<Option<Value>, ProviderError> {
        match self.execute(&qom_get_request(
            &format!("{PERIPHERAL_PATH}/{id}"),
            property,
        )) {
            Ok(value) => Ok(Some(value)),
            Err(QmpError::Command(_)) => Ok(None),
            Err(error) => Err(map_qmp_failure("device inspection", error)),
        }
    }

    /// The monitor's `info usb` text, or `None` on a QEMU too old to expose it over QMP.
    pub fn usb_summary(&mut self) -> Result<Option<String>, ProviderError> {
        match self.execute(&x_query_usb_request()) {
            Ok(value) => Ok(value
                .get("human-readable-text")
                .and_then(Value::as_str)
                .map(str::to_string)),
            Err(QmpError::Command(failure)) if failure.class == "CommandNotFound" => Ok(None),
            Err(error) => Err(map_qmp_failure("USB listing", error)),
        }
    }
}

fn transport_error(error: QmpError) -> ProviderError {
    match error {
        QmpError::Transport(message) => ProviderError::UsbPassthrough(message),
        QmpError::Command(failure) => ProviderError::UsbPassthrough(format!(
            "QMP refused the handshake ({}): {}",
            failure.class, failure.desc
        )),
    }
}

pub(crate) fn capabilities_request() -> Value {
    json!({ "execute": "qmp_capabilities" })
}

/// `hostbus`/`hostport` matching means the port, not the device, is handed to the guest: an
/// unplug and replug on the same port re-attaches without another command.
///
/// The phone goes on the machine's dedicated EHCI controller — on the emulated xHCI macOS never
/// assigns an iPhone an address — and whether the guest may reset it depends on the guest, see
/// [`guest_reset_for_macos`]. One caveat governs the whole design: QEMU reads a phone cleanly
/// only the first time it opens it in a process, so a phone that has been detached must be
/// unplugged and plugged in again before it is attached again, and a phone QEMU has lost track
/// of needs the machine restarted.
/// The same device handed over by its device node. QEMU opens the node itself and never
/// consults libusb's device list, which matters in a container whose libusb learns of a phone
/// only through hot-plug events it never receives: after the phone re-enumerates itself, which
/// an iPhone does once when a host first configures it, the node is the only way to reach it.
pub(crate) fn device_add_by_node_request(node: &str, guest_reset: bool) -> Value {
    json!({
        "execute": "device_add",
        "arguments": {
            "driver": "usb-host",
            "id": IPHONE_QMP_DEVICE_ID,
            "bus": format!("{USB_PHONE_CONTROLLER}.0"),
            "hostdevice": node,
            "guest-reset": guest_reset
        }
    })
}

pub(crate) fn device_add_request(bus: u8, port: &str, guest_reset: bool) -> Value {
    json!({
        "execute": "device_add",
        "arguments": {
            "driver": "usb-host",
            "id": IPHONE_QMP_DEVICE_ID,
            "bus": format!("{USB_PHONE_CONTROLLER}.0"),
            "hostbus": bus,
            "hostport": port,
            "guest-reset": guest_reset
        }
    })
}

/// Whether the guest may really reset the phone, by the guest's macOS version. Measured twice
/// each on a phone on the desk: macOS 15.7 addresses a phone it may not reset but never
/// configures it, and configures one it may reset within ten seconds; macOS 26 resets a phone
/// it may reset into re-enumerating on the host, which leaves QEMU holding a dead handle, and
/// configures one it may not reset within two seconds. Unknown versions get the newer
/// behaviour, since that is what a fresh install is.
pub fn guest_reset_for_macos(macos_version: Option<&str>) -> bool {
    let major: u32 = macos_version
        .and_then(|version| version.split('.').next())
        .and_then(|major| major.trim().parse().ok())
        .unwrap_or(GUEST_RESET_CUTOFF_MAJOR);

    major < GUEST_RESET_CUTOFF_MAJOR
}

/// The first macOS major that must not really reset the phone.
const GUEST_RESET_CUTOFF_MAJOR: u32 = 26;

pub(crate) fn device_del_request(id: &str) -> Value {
    json!({ "execute": "device_del", "arguments": { "id": id } })
}

pub(crate) fn qom_list_request(path: &str) -> Value {
    json!({ "execute": "qom-list", "arguments": { "path": path } })
}

pub(crate) fn qom_get_request(path: &str, property: &str) -> Value {
    json!({ "execute": "qom-get", "arguments": { "path": path, "property": property } })
}

pub(crate) fn x_query_usb_request() -> Value {
    json!({ "execute": "x-query-usb" })
}

pub(crate) fn classify_response(value: &Value) -> QmpMessage {
    let Some(object) = value.as_object() else {
        return QmpMessage::Unknown;
    };
    if object.contains_key("QMP") {
        return QmpMessage::Greeting;
    }
    if let Some(returned) = object.get("return") {
        return QmpMessage::Return(returned.clone());
    }
    if let Some(error) = object.get("error") {
        return QmpMessage::Error(QmpFailure {
            class: error
                .get("class")
                .and_then(Value::as_str)
                .unwrap_or("GenericError")
                .to_string(),
            desc: error
                .get("desc")
                .and_then(Value::as_str)
                .unwrap_or("no description")
                .to_string(),
        });
    }
    if object.contains_key("event") {
        return QmpMessage::Event;
    }

    QmpMessage::Unknown
}

/// The names of the devices under `/machine/peripheral`: every `-device …,id=` object.
pub(crate) fn parse_peripheral_ids(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter(|item| {
                    item.get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| kind.starts_with("child<"))
                })
                .filter_map(|item| item.get("name").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// `info usb` lists an attached device as `Device 0.2, Port 1, …, Product iPhone, ID: <id>`
/// only once QEMU has opened the host device and the guest has enumerated it.
/// How QEMU is holding the passed-through device.
///
/// QEMU lists the device id as soon as it owns the host port, whether or not it could read the
/// phone. A phone that was reset mid-handover comes back as a low-speed `USB Host Device` with
/// no readable descriptors: QEMU still names it, the guest never enumerates it, and only a
/// physical replug recovers it. Treating that as attached is what made the interface claim a
/// phone was ready while macOS had nothing, so the two cases are told apart here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UsbAttachment {
    /// QEMU does not hold the port.
    Absent,
    /// QEMU holds the port but could not read the device.
    Unreadable,
    /// QEMU read the device's own descriptors.
    Live,
}

/// QEMU's placeholder product name for a device whose descriptors it could not read.
const UNREADABLE_PRODUCT: &str = "Product USB Host Device";
/// The speed a reset phone falls back to; a real iPhone runs at high speed or better.
const UNREADABLE_SPEED: &str = "Speed 1.5 Mb/s";

pub(crate) fn usb_attachment(text: &str, id: &str) -> UsbAttachment {
    let marker = format!("ID: {id}");
    let Some(line) = text.lines().find(|line| line.contains(&marker)) else {
        return UsbAttachment::Absent;
    };
    if line.contains(UNREADABLE_PRODUCT) || line.contains(UNREADABLE_SPEED) {
        return UsbAttachment::Unreadable;
    }

    UsbAttachment::Live
}

pub(crate) fn map_qmp_failure(operation: &'static str, error: QmpError) -> ProviderError {
    match error {
        QmpError::Transport(message) => {
            ProviderError::UsbPassthrough(format!("QMP {operation} failed: {message}"))
        }
        QmpError::Command(failure) => {
            let description = failure.desc.to_ascii_lowercase();
            let message = if description.contains("duplicate id") {
                "the phone is already attached to this machine".to_string()
            } else if description.contains("libusb_error_access")
                || description.contains("permission denied")
            {
                "the container cannot open the phone; check the USB rule, then unplug and plug the phone in again".to_string()
            } else if description.contains("libusb_error_busy") {
                "another program on this computer is holding the phone".to_string()
            } else {
                format!(
                    "QMP {operation} failed ({}): {}",
                    failure.class, failure.desc
                )
            };

            ProviderError::UsbPassthrough(message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qmp_requests_are_fixed_json_with_the_iphone_device_id() {
        assert_eq!(
            capabilities_request(),
            json!({ "execute": "qmp_capabilities" })
        );
        assert_eq!(
            device_add_request(3, "2.3.1", false),
            json!({
                "execute": "device_add",
                "arguments": {
                    "driver": "usb-host",
                    "id": "buildbridge-iphone",
                    "bus": "buildbridge-phone-usb.0",
                    "hostbus": 3,
                    "hostport": "2.3.1",
                    "guest-reset": false
                }
            })
        );
        assert_eq!(
            device_add_by_node_request("/dev/bus/usb/003/012", false),
            json!({
                "execute": "device_add",
                "arguments": {
                    "driver": "usb-host",
                    "id": "buildbridge-iphone",
                    "bus": "buildbridge-phone-usb.0",
                    "hostdevice": "/dev/bus/usb/003/012",
                    "guest-reset": false
                }
            })
        );
        // The guest's macOS version decides the reset: 15 may, 26 may not, unknown is treated
        // as new.
        assert!(guest_reset_for_macos(Some("15.7.9")));
        assert!(guest_reset_for_macos(Some("14.5")));
        assert!(!guest_reset_for_macos(Some("26.6.2")));
        assert!(!guest_reset_for_macos(Some("27.0")));
        assert!(!guest_reset_for_macos(None));
        assert!(!guest_reset_for_macos(Some("garbage")));
        assert_eq!(
            device_del_request("buildbridge-iphone"),
            json!({ "execute": "device_del", "arguments": { "id": "buildbridge-iphone" } })
        );
        assert_eq!(
            qom_list_request("/machine/peripheral"),
            json!({ "execute": "qom-list", "arguments": { "path": "/machine/peripheral" } })
        );
        assert_eq!(
            qom_get_request("/machine/peripheral/buildbridge-iphone", "hostport"),
            json!({
                "execute": "qom-get",
                "arguments": { "path": "/machine/peripheral/buildbridge-iphone", "property": "hostport" }
            })
        );
        assert_eq!(x_query_usb_request(), json!({ "execute": "x-query-usb" }));
    }

    #[test]
    fn qmp_responses_separate_greetings_returns_errors_and_events() {
        assert_eq!(
            classify_response(&json!({ "QMP": { "version": {}, "capabilities": [] } })),
            QmpMessage::Greeting
        );
        assert_eq!(
            classify_response(&json!({ "return": {} })),
            QmpMessage::Return(json!({}))
        );
        assert_eq!(
            classify_response(
                &json!({ "error": { "class": "DeviceNotFound", "desc": "Device 'x' not found" } })
            ),
            QmpMessage::Error(QmpFailure {
                class: "DeviceNotFound".to_string(),
                desc: "Device 'x' not found".to_string(),
            })
        );
        assert_eq!(
            classify_response(&json!({ "event": "DEVICE_DELETED", "data": {} })),
            QmpMessage::Event
        );
        assert_eq!(classify_response(&json!("nonsense")), QmpMessage::Unknown);
    }

    #[test]
    fn attached_iphone_is_read_from_the_peripheral_listing() {
        let listing = json!([
            { "name": "type", "type": "string" },
            { "name": "buildbridge-iphone", "type": "child<usb-host>" },
            { "name": "net0", "type": "child<vmxnet3>" }
        ]);

        assert_eq!(
            parse_peripheral_ids(&listing),
            vec!["buildbridge-iphone".to_string(), "net0".to_string()]
        );
        assert!(parse_peripheral_ids(&json!({})).is_empty());
    }

    #[test]
    fn x_query_usb_text_reveals_the_enumerated_iphone() {
        let text = "  Device 0.2, Port 1, Speed 480 Mb/s, Product QEMU USB Keyboard\n  Device 0.3, Port 2, Speed 5000 Mb/s, Product iPhone, ID: buildbridge-iphone\n";

        assert_eq!(
            usb_attachment(text, "buildbridge-iphone"),
            UsbAttachment::Live
        );
        assert_eq!(
            usb_attachment(
                "  Device 0.2, Port 1, Speed 480 Mb/s, Product QEMU USB Tablet\n",
                "buildbridge-iphone"
            ),
            UsbAttachment::Absent
        );
    }

    #[test]
    fn a_phone_reset_during_handover_is_not_reported_as_attached() {
        // Observed on a real phone after QEMU reset it: named, but no readable descriptors.
        let degraded = "  Device 0.2, Port 1, Speed 480 Mb/s, Product QEMU USB Keyboard\n  Device 0.0, Port 4, Speed 1.5 Mb/s, Product USB Host Device, ID: buildbridge-iphone\n";

        assert_eq!(
            usb_attachment(degraded, "buildbridge-iphone"),
            UsbAttachment::Unreadable
        );
        // A high-speed line that merely lacks a product name is still a real claim.
        assert_eq!(
            usb_attachment(
                "  Device 0.3, Port 3, Speed 480 Mb/s, Product iPhone, ID: buildbridge-iphone\n",
                "buildbridge-iphone"
            ),
            UsbAttachment::Live
        );
    }

    #[test]
    fn qmp_error_classes_map_to_actionable_messages() {
        let duplicate = map_qmp_failure(
            "device attach",
            QmpError::Command(QmpFailure {
                class: "GenericError".to_string(),
                desc: "Duplicate ID 'buildbridge-iphone' for device".to_string(),
            }),
        );
        assert!(duplicate.to_string().contains("already attached"));

        let access = map_qmp_failure(
            "device attach",
            QmpError::Command(QmpFailure {
                class: "GenericError".to_string(),
                desc: "libusb_open: LIBUSB_ERROR_ACCESS".to_string(),
            }),
        );
        assert!(access.to_string().contains("USB rule"));

        let busy = map_qmp_failure(
            "device attach",
            QmpError::Command(QmpFailure {
                class: "GenericError".to_string(),
                desc: "LIBUSB_ERROR_BUSY".to_string(),
            }),
        );
        assert!(busy.to_string().contains("holding the phone"));

        let other = map_qmp_failure(
            "device listing",
            QmpError::Transport("QEMU closed the control socket".to_string()),
        );
        assert!(other.to_string().contains("device listing"));
    }
}
