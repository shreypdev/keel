//! `adb`, for `undra dev --android`: makes the dev server's port reachable from the Android
//! devices and emulators that are attached.
//!
//! An emulator reaches this machine as `10.0.2.2` and needs nothing. A USB device (or an
//! emulator, for that matter) reaches it at `127.0.0.1` once `adb reverse tcp:<port> tcp:<port>`
//! has said so, which is what [`reverse_all`] runs for every device `adb devices` lists as ready
//! (or for the one named by `ANDROID_SERIAL`, as `adb` itself does).

use std::path::{Path, PathBuf};

use crate::sys::Sys;
use crate::toolchain::Toolchain;

/// One line of `adb devices`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// The serial (`emulator-5554`, `R5CT1234ABC`).
    pub serial: String,
    /// `device` when it is ready; `offline`, `unauthorized`, ... when it is not.
    pub state: String,
}

impl Device {
    /// Whether commands can be sent to it.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.state == "device"
    }
}

/// What reversing the port did for one device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// The device.
    pub serial: String,
    /// `Ok` when `adb reverse` succeeded, else what `adb` said.
    pub result: Result<(), String>,
}

/// Everything [`reverse_all`] found out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reversal {
    /// One entry per ready device.
    pub reversed: Vec<Outcome>,
    /// Devices that are attached but not ready, with their state.
    pub not_ready: Vec<Device>,
    /// Why nothing could be done at all (no `adb`, `adb devices` failed, no device).
    pub problem: Option<String>,
}

/// Parses the output of `adb devices`.
#[must_use]
pub fn parse_devices(output: &str) -> Vec<Device> {
    output
        .lines()
        .filter(|line| !line.starts_with("List of devices") && !line.starts_with('*'))
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let serial = parts.next()?;
            let state = parts.next()?;
            Some(Device {
                serial: serial.to_owned(),
                state: state.to_owned(),
            })
        })
        .collect()
}

/// Finds `adb`: on `PATH`, else in the Android SDK's `platform-tools`.
#[must_use]
pub fn find(sys: &dyn Sys, toolchain: &Toolchain) -> Option<PathBuf> {
    if let Some(found) = toolchain.which(sys, "adb") {
        return Some(found);
    }
    let sdk = toolchain.android_sdk.as_ref()?;
    let candidate = sdk
        .join("platform-tools")
        .join(if cfg!(windows) { "adb.exe" } else { "adb" });
    sys.is_file(&candidate).then_some(candidate)
}

/// Runs `adb reverse tcp:<port> tcp:<port>` for every ready device (only `only`, when it names
/// one: `ANDROID_SERIAL`).
#[must_use]
pub fn reverse_all(
    sys: &dyn Sys,
    toolchain: &Toolchain,
    port: u16,
    only: Option<&str>,
) -> Reversal {
    let mut result = Reversal::default();
    let Some(adb) = find(sys, toolchain) else {
        result.problem = Some(
            "adb was not found (it is in the Android SDK's platform-tools; `undra doctor` checks the SDK)"
                .to_owned(),
        );
        return result;
    };
    let env = toolchain.env_pairs();
    let Some(listing) = sys.run(&adb, &["devices"], &env).filter(|o| o.success) else {
        result.problem = Some("`adb devices` failed".to_owned());
        return result;
    };
    let devices: Vec<Device> = parse_devices(&listing.stdout)
        .into_iter()
        .filter(|d| only.is_none_or(|serial| d.serial == serial))
        .collect();
    if devices.is_empty() {
        result.problem = Some(match only {
            Some(serial) => format!("no attached device is called {serial} (ANDROID_SERIAL)"),
            None => "no Android device or emulator is attached".to_owned(),
        });
        return result;
    }
    let spec = format!("tcp:{port}");
    for device in devices {
        if !device.is_ready() {
            result.not_ready.push(device);
            continue;
        }
        result.reversed.push(Outcome {
            result: reverse_one(sys, &adb, &env, &device.serial, &spec),
            serial: device.serial,
        });
    }
    result
}

fn reverse_one(
    sys: &dyn Sys,
    adb: &Path,
    env: &[(String, String)],
    serial: &str,
    spec: &str,
) -> Result<(), String> {
    match sys.run(adb, &["-s", serial, "reverse", spec, spec], env) {
        Some(out) if out.success => Ok(()),
        Some(out) => Err(out.text()),
        None => Err("could not start adb".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::CmdOutput;
    use crate::sys::fake::FakeSys;

    const ADB: &str = "/sdk/platform-tools/adb";

    fn with_adb(devices: &str) -> FakeSys {
        FakeSys::macos()
            .with_tool("adb", ADB)
            .with_output("adb", "devices", devices)
    }

    #[test]
    fn devices_are_parsed_from_the_listing() {
        let listing = "List of devices attached\n* daemon not running; starting now at tcp:5037\n* daemon started successfully\nemulator-5554\tdevice\nR5CT1234ABC\tunauthorized\n0123\toffline\n\n";
        let devices = parse_devices(listing);
        assert_eq!(
            devices,
            [
                Device {
                    serial: "emulator-5554".into(),
                    state: "device".into()
                },
                Device {
                    serial: "R5CT1234ABC".into(),
                    state: "unauthorized".into()
                },
                Device {
                    serial: "0123".into(),
                    state: "offline".into()
                },
            ]
        );
        assert!(devices[0].is_ready());
        assert!(!devices[1].is_ready());
        assert!(parse_devices("List of devices attached\n\n").is_empty());
    }

    #[test]
    fn every_ready_device_gets_its_port_reversed_and_the_others_are_reported() {
        let sys = with_adb("List of devices attached\nemulator-5554\tdevice\nphone1\tdevice\nphone2\tunauthorized\n")
            .with_output("adb", "-s emulator-5554 reverse tcp:7443 tcp:7443", "")
            .with_output("adb", "-s phone1 reverse tcp:7443 tcp:7443", "7443\n");
        let result = reverse_all(&sys, &Toolchain::default(), 7443, None);
        assert_eq!(result.problem, None);
        assert_eq!(
            result.reversed,
            [
                Outcome {
                    serial: "emulator-5554".into(),
                    result: Ok(())
                },
                Outcome {
                    serial: "phone1".into(),
                    result: Ok(())
                },
            ]
        );
        assert_eq!(
            result.not_ready,
            [Device {
                serial: "phone2".into(),
                state: "unauthorized".into()
            }]
        );
    }

    #[test]
    fn android_serial_limits_it_to_one_device() {
        let sys =
            with_adb("List of devices attached\nemulator-5554\tdevice\nemulator-5580\tdevice\n")
                .with_output("adb", "-s emulator-5554 reverse tcp:9000 tcp:9000", "");
        let one = reverse_all(&sys, &Toolchain::default(), 9000, Some("emulator-5554"));
        assert_eq!(one.reversed.len(), 1);
        assert_eq!(one.reversed[0].serial, "emulator-5554");
        let none = reverse_all(&sys, &Toolchain::default(), 9000, Some("nope"));
        assert!(none.reversed.is_empty());
        assert!(none.problem.unwrap().contains("nope"));
    }

    #[test]
    fn a_failing_reverse_carries_what_adb_said() {
        let mut sys = with_adb("List of devices attached\nphone1\tdevice\n");
        sys.outputs.insert(
            "adb -s phone1 reverse tcp:7443 tcp:7443".to_owned(),
            CmdOutput {
                success: false,
                stdout: String::new(),
                stderr: "error: closed".to_owned(),
            },
        );
        let result = reverse_all(&sys, &Toolchain::default(), 7443, None);
        assert_eq!(result.reversed[0].result, Err("error: closed".to_owned()));
    }

    #[test]
    fn no_adb_and_no_device_are_problems_with_words() {
        let none = reverse_all(&FakeSys::macos(), &Toolchain::default(), 7443, None);
        assert!(none.problem.unwrap().contains("adb was not found"));
        let empty = reverse_all(
            &with_adb("List of devices attached\n\n"),
            &Toolchain::default(),
            7443,
            None,
        );
        assert!(
            empty
                .problem
                .unwrap()
                .contains("no Android device or emulator is attached")
        );
    }

    #[test]
    fn adb_is_found_in_the_sdk_when_it_is_not_on_the_path() {
        let sys = FakeSys::macos().with_file("/sdk/platform-tools/adb");
        let toolchain = Toolchain {
            android_sdk: Some(PathBuf::from("/sdk")),
            ..Toolchain::default()
        };
        assert_eq!(find(&sys, &toolchain), Some(PathBuf::from(ADB)));
        assert_eq!(find(&FakeSys::macos(), &toolchain), None);
    }
}
