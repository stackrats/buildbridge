//! A small QMP client for the machine's QEMU: JSON lines over the unix socket QEMU exposes
//! through the bind-mounted control directory.
//!
//! It exists to plug a host USB device into the guest and to ask what the guest sees. It never
//! touches the disk, the network, or the monitor's human-readable commands beyond the one
//! `x-query-usb` summary, and every request is built from validated values.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::ProviderError;

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

pub(crate) struct QmpClient {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
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
    pub fn connect(socket: &Path) -> Result<Self, ProviderError> {
        let stream = UnixStream::connect(socket).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                ProviderError::UsbPassthrough(
                    "the machine is not running, so its QEMU control socket is closed".to_string(),
                )
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
        let mut client = Self {
            reader: BufReader::new(stream),
            writer,
        };

        let mut greeted = false;
        for _ in 0..8 {
            match client.read_message().map_err(transport_error)? {
                QmpMessage::Greeting => {
                    greeted = true;
                    break;
                }
                QmpMessage::Event => continue,
                _ => break,
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

    fn read_message(&mut self) -> Result<QmpMessage, QmpError> {
        let mut line = String::new();
        let read = self
            .reader
            .read_line(&mut line)
            .map_err(|error| QmpError::Transport(error.to_string()))?;
        if read == 0 {
            return Err(QmpError::Transport(
                "QEMU closed the control socket".to_string(),
            ));
        }
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

    pub fn add_usb_host(&mut self, bus: u8, port: &str) -> Result<(), ProviderError> {
        self.execute(&device_add_request(bus, port))
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
/// The phone goes on the machine's dedicated EHCI controller, and the guest is allowed to reset
/// it. Both were measured on a phone on the desk: on the emulated xHCI macOS never assigns the
/// phone an address, and with the reset refused it addresses the phone but never configures it.
/// With both as they are here, macOS selects the phone's NCM configuration within ten seconds
/// and registers it under a minute later, with nothing restarted. One caveat governs the whole
/// design: QEMU reads a phone cleanly only the first time it opens it in a process, so a phone
/// that has been detached must be unplugged and plugged in again before it is attached again.
pub(crate) fn device_add_request(bus: u8, port: &str) -> Value {
    json!({
        "execute": "device_add",
        "arguments": {
            "driver": "usb-host",
            "id": IPHONE_QMP_DEVICE_ID,
            "bus": format!("{USB_PHONE_CONTROLLER}.0"),
            "hostbus": bus,
            "hostport": port
        }
    })
}

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
            device_add_request(3, "2.3.1"),
            json!({
                "execute": "device_add",
                "arguments": {
                    "driver": "usb-host",
                    "id": "buildbridge-iphone",
                    "bus": "buildbridge-phone-usb.0",
                    "hostbus": 3,
                    "hostport": "2.3.1"
                }
            })
        );
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
