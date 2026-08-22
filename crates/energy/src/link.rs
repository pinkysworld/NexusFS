//! Deciding whether the active network link is paid for by the byte.
//!
//! The scheduler has always treated a metered link as an override of the battery
//! grade — no amount of remaining charge makes a per-byte bill acceptable. Nothing ever
//! reported one, so that rule could only fire in a test. This is the reporting half.
//!
//! # Why this is partial, and says so
//!
//! "Metered" is not a property the kernel knows. It is a policy statement about a
//! network, and only some systems record it:
//!
//! - **Linux** with NetworkManager keeps it per connection, including a guess for link
//!   types it believes are cellular. That is a real answer in both directions.
//! - **macOS** exposes no general equivalent. What is detectable is the common case —
//!   the machine is tethered to a phone over USB — and nothing more. A Mac joined to an
//!   iPhone's Wi-Fi hotspot is indistinguishable from any other Wi-Fi network here.
//! - **Everything else** reports unknown.
//!
//! A VPN defeats all of it, on every platform. Detection follows the default route, and
//! with a tunnel up that route points at `utun`/`tun`, whose cost is a property of the
//! physical link underneath it. Recovering that means resolving the route to the VPN's
//! own endpoint, which is a much larger job than this rule is worth.
//!
//! So detection alone would leave the rule unusable for many real deployments, which is
//! why [`LinkCost`] can also be stated in config. An operator who knows they are on a
//! satellite uplink — or behind a VPN on a mobile plan — should be able to say so
//! without waiting for a probe to be written for their platform.
//!
//! Throughout, an unreadable or absent source yields [`LinkCost::Unknown`], never
//! [`LinkCost::Unmetered`]. The difference matters: "we asked and it is not metered" and
//! "we have no way to ask" are the same instruction to the scheduler today, but only one
//! of them is a fact, and reporting a guess as a fact is how a console starts lying.

use crate::telemetry::LinkCost;

/// Read the link cost from the operating system, as far as it can be known.
pub fn detect() -> LinkCost {
    platform_detect()
}

/// Parse the `energy.link_cost` config value.
///
/// `auto` (or absent) means detect; anything else states the answer outright and skips
/// detection entirely, because an operator's declaration is not a hypothesis to check.
/// An unrecognised value is treated as `auto` by the caller, which reports it.
pub fn parse_config(raw: &str) -> Option<LinkCost> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "metered" => Some(LinkCost::Metered),
        "unmetered" => Some(LinkCost::Unmetered),
        "unknown" => Some(LinkCost::Unknown),
        _ => None,
    }
}

/// Whether `raw` names a link cost this build understands, `auto` included.
pub fn config_is_valid(raw: &str) -> bool {
    let v = raw.trim().to_ascii_lowercase();
    v.is_empty() || v == "auto" || parse_config(&v).is_some()
}

// --- Linux -------------------------------------------------------------------

/// The interface carrying the default route, from the contents of `/proc/net/route`.
///
/// Chosen by lowest metric rather than first match: a machine with both Wi-Fi and a
/// tether up has two default routes, and the one with the lower metric is the one
/// traffic will actually take. Picking the first would report the cost of a link that
/// is not carrying anything.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn default_route_iface(proc_net_route: &str) -> Option<&str> {
    proc_net_route
        .lines()
        .skip(1) // header
        .filter_map(|line| {
            let mut cols = line.split_whitespace();
            let iface = cols.next()?;
            let destination = cols.next()?;
            // A destination of all zeroes is the default route.
            if destination != "00000000" {
                return None;
            }
            let metric = cols.nth(4).and_then(|m| m.parse::<u32>().ok())?;
            Some((metric, iface))
        })
        .min_by_key(|(metric, _)| *metric)
        .map(|(_, iface)| iface)
}

/// Read `GENERAL.METERED` out of terse `nmcli` output.
///
/// NetworkManager answers with `yes`, `no`, `unknown`, or either of the first two
/// marked `(guessed)`. The guesses are honoured rather than discarded: they are how NM
/// reports that a link is cellular, which is exactly the case this rule exists for, and
/// the cost of believing a wrong guess is deferred bytes that a read will now fetch
/// anyway.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn parse_nm_metered(out: &str) -> LinkCost {
    let Some(value) = out
        .lines()
        .find_map(|l| l.trim().strip_prefix("GENERAL.METERED:"))
    else {
        return LinkCost::Unknown;
    };

    let value = value.trim().to_ascii_lowercase();
    // Checked before `no`, because "unknown" contains neither and a bare prefix test
    // against "no" would also match nothing useful.
    if value.starts_with("yes") {
        LinkCost::Metered
    } else if value.starts_with("no") {
        LinkCost::Unmetered
    } else {
        LinkCost::Unknown
    }
}

#[cfg(target_os = "linux")]
fn platform_detect() -> LinkCost {
    let Ok(routes) = std::fs::read_to_string("/proc/net/route") else {
        return LinkCost::Unknown;
    };
    let Some(iface) = default_route_iface(&routes) else {
        // No default route: nothing is going anywhere, so there is no link to price.
        return LinkCost::Unknown;
    };

    // Costs one subprocess per sample. Metered state is a NetworkManager concept with
    // no sysfs equivalent, so there is nothing cheaper to read; a machine without
    // NetworkManager fails the spawn and reports unknown, which is correct.
    let Ok(out) = std::process::Command::new("nmcli")
        .args(["-t", "-f", "GENERAL.METERED", "device", "show", iface])
        .output()
    else {
        return LinkCost::Unknown;
    };
    if !out.status.success() {
        return LinkCost::Unknown;
    }

    parse_nm_metered(&String::from_utf8_lossy(&out.stdout))
}

// --- macOS -------------------------------------------------------------------

/// The interface named by `route -n get default`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn parse_route_get_default(out: &str) -> Option<&str> {
    out.lines()
        .find_map(|l| l.trim().strip_prefix("interface:"))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Whether `networksetup -listallhardwareports` shows `iface` as a phone tether.
///
/// The output is blocks of `Hardware Port:` / `Device:` lines, so this tracks the most
/// recent port name and reports it when the device matches. Only tethering is
/// detectable: a hotspot joined over Wi-Fi presents as ordinary Wi-Fi, and guessing
/// from the network's name would be a heuristic dressed up as a reading.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn is_tether_port(out: &str, iface: &str) -> bool {
    let mut port = "";
    for line in out.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix("Hardware Port:") {
            port = name.trim();
        } else if let Some(device) = line.strip_prefix("Device:") {
            if device.trim() == iface {
                let port = port.to_ascii_lowercase();
                return port.contains("iphone") || port.contains("ipad");
            }
        }
    }
    false
}

#[cfg(target_os = "macos")]
fn platform_detect() -> LinkCost {
    let Ok(route) = std::process::Command::new("route")
        .args(["-n", "get", "default"])
        .output()
    else {
        return LinkCost::Unknown;
    };
    let route = String::from_utf8_lossy(&route.stdout);
    let Some(iface) = parse_route_get_default(&route) else {
        return LinkCost::Unknown;
    };

    let Ok(ports) = std::process::Command::new("networksetup")
        .arg("-listallhardwareports")
        .output()
    else {
        return LinkCost::Unknown;
    };

    if is_tether_port(&String::from_utf8_lossy(&ports.stdout), iface) {
        LinkCost::Metered
    } else {
        // Not a tether. That is not the same as knowing the link is free — a Wi-Fi
        // hotspot looks identical to home broadband from here — so this stays unknown
        // and the operator can state it in config if it matters.
        LinkCost::Unknown
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_detect() -> LinkCost {
    LinkCost::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_values_are_read_case_insensitively() {
        assert_eq!(parse_config("metered"), Some(LinkCost::Metered));
        assert_eq!(parse_config("  Metered "), Some(LinkCost::Metered));
        assert_eq!(parse_config("UNMETERED"), Some(LinkCost::Unmetered));
        assert_eq!(parse_config("unknown"), Some(LinkCost::Unknown));
        assert_eq!(parse_config("auto"), None, "auto means detect");
        assert_eq!(parse_config("nonsense"), None);
    }

    #[test]
    fn auto_and_empty_are_valid_config_but_nonsense_is_not() {
        assert!(config_is_valid("auto"));
        assert!(config_is_valid(""));
        assert!(config_is_valid("metered"));
        assert!(!config_is_valid("cheap"));
    }

    #[test]
    fn the_default_route_is_picked_by_lowest_metric() {
        // Both a tether and Wi-Fi are up. Traffic takes the lower metric, so that is the
        // link whose cost matters; reading the first row would price the wrong one.
        let routes = "\
Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT
wlan0\t00000000\t0102A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0
usb0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0
wlan0\t0002A8C0\t00000000\t0001\t0\t0\t600\t00FFFFFF\t0\t0\t0
";
        assert_eq!(default_route_iface(routes), Some("usb0"));
    }

    #[test]
    fn a_table_with_no_default_route_names_nothing() {
        let routes = "\
Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT
wlan0\t0002A8C0\t00000000\t0001\t0\t0\t600\t00FFFFFF\t0\t0\t0
";
        assert_eq!(default_route_iface(routes), None);
        assert_eq!(default_route_iface(""), None);
    }

    #[test]
    fn network_manager_answers_are_read_in_both_directions() {
        assert_eq!(parse_nm_metered("GENERAL.METERED:yes\n"), LinkCost::Metered);
        assert_eq!(
            parse_nm_metered("GENERAL.METERED:no\n"),
            LinkCost::Unmetered
        );
    }

    #[test]
    fn a_guessed_answer_is_still_an_answer() {
        // How NetworkManager reports a link it believes is cellular — the case this
        // whole rule exists for.
        assert_eq!(
            parse_nm_metered("GENERAL.METERED:yes (guessed)\n"),
            LinkCost::Metered
        );
        assert_eq!(
            parse_nm_metered("GENERAL.METERED:no (guessed)\n"),
            LinkCost::Unmetered
        );
    }

    #[test]
    fn an_absent_or_unknown_field_never_reads_as_unmetered() {
        // The distinction the module docs turn on: not knowing must not be reported as
        // knowing the link is free.
        assert_eq!(
            parse_nm_metered("GENERAL.METERED:unknown\n"),
            LinkCost::Unknown
        );
        assert_eq!(parse_nm_metered(""), LinkCost::Unknown);
        assert_eq!(
            parse_nm_metered("Error: Device 'x' not found.\n"),
            LinkCost::Unknown
        );
    }

    #[test]
    fn the_macos_default_interface_is_read_from_route_output() {
        let out = "\
   route to: default
destination: default
       mask: default
    gateway: 192.168.1.1
  interface: en0
      flags: <UP,GATEWAY,DONE,STATIC,PRCLONING,GLOBAL>
";
        assert_eq!(parse_route_get_default(out), Some("en0"));
        assert_eq!(parse_route_get_default("route: no route to host"), None);
    }

    const HARDWARE_PORTS: &str = "\
Hardware Port: Wi-Fi
Device: en0
Ethernet Address: aa:bb:cc:dd:ee:ff

Hardware Port: iPhone USB
Device: en5
Ethernet Address: 11:22:33:44:55:66

Hardware Port: Thunderbolt Bridge
Device: bridge0
Ethernet Address: 77:88:99:aa:bb:cc
";

    #[test]
    fn a_phone_tether_is_recognised_by_its_hardware_port() {
        assert!(is_tether_port(HARDWARE_PORTS, "en5"));
    }

    #[test]
    fn ordinary_ports_are_not_tethers() {
        // Wi-Fi is the interesting one: it may well *be* a hotspot, and this
        // deliberately does not guess.
        assert!(!is_tether_port(HARDWARE_PORTS, "en0"));
        assert!(!is_tether_port(HARDWARE_PORTS, "bridge0"));
        assert!(!is_tether_port(HARDWARE_PORTS, "en9"));
    }

    #[test]
    fn a_vpn_tunnel_is_not_a_hardware_port_and_reports_nothing() {
        // With a tunnel up the default route names a `utun`, which no hardware port
        // matches — so this reports "not a tether" and the caller reports unknown,
        // rather than concluding the physical link underneath is free. The documented
        // limitation, pinned so it cannot quietly become a claim.
        assert!(!is_tether_port(HARDWARE_PORTS, "utun8"));
    }

    #[test]
    fn a_hardware_port_named_after_its_device_does_not_confuse_the_match() {
        // Real macOS output includes ports like "Ethernet Adapter (en3)", where the
        // port name contains an interface name. Matching must key off the Device line.
        let out = "\
Hardware Port: Ethernet Adapter (en3)
Device: en3
Ethernet Address: 42:d8:a3:b5:b0:66

Hardware Port: iPhone USB
Device: en7
Ethernet Address: 11:22:33:44:55:66
";
        assert!(!is_tether_port(out, "en3"));
        assert!(is_tether_port(out, "en7"));
    }
}
