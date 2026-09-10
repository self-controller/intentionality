//! Which microphones PipeWire will admit to, for the selector on the Meetings
//! screen.
//!
//! A module of its own rather than a body inside commands.rs, which is
//! deliberately thin — this shells out to `pw-dump`, parses its JSON and has
//! its own tests, the same shape as aw.rs probing ActivityWatch.
//!
//! Every failure here is an empty list, never an error. A machine with no
//! PipeWire, a `pw-dump` that is missing or hangs, output that is not the JSON
//! this expects: all of them mean "offer System default only", because none of
//! them are a reason the Meetings screen should refuse to load. The detail
//! goes to the journal instead.

use crate::models::AudioSource;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// `pw-dump` on a healthy machine returns in milliseconds. This is only here
/// so a wedged PipeWire cannot hold the Meetings screen open forever.
const TIMEOUT: Duration = Duration::from_secs(3);

pub async fn sources() -> Vec<AudioSource> {
    match tokio::time::timeout(TIMEOUT, run()).await {
        Ok(Ok(json)) => parse(&json),
        Ok(Err(err)) => {
            eprintln!("meeting: could not list audio sources ({err}) — offering the default only");
            Vec::new()
        }
        Err(_) => {
            eprintln!("meeting: pw-dump did not answer within {TIMEOUT:?} — offering the default only");
            Vec::new()
        }
    }
}

async fn run() -> std::io::Result<String> {
    let out = Command::new("pw-dump")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await?;
    if !out.status.success() {
        return Err(std::io::Error::other(format!("pw-dump exited {}", out.status)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Pull the capture nodes out of a `pw-dump` document.
///
/// The `target` returned is `node.name`, not the object serial: `pw-record
/// --target` takes either, but a serial is assigned per connection and changes
/// when a device is unplugged and plugged back in, so it is worthless as
/// something to remember. The label is for people and is never passed to
/// pw-record.
fn parse(json: &str) -> Vec<AudioSource> {
    let Ok(objects) = serde_json::from_str::<Vec<serde_json::Value>>(json) else {
        eprintln!("meeting: pw-dump output was not the expected JSON array");
        return Vec::new();
    };
    let mut out = Vec::new();
    for object in &objects {
        let props = &object["info"]["props"];
        if props["media.class"].as_str() != Some("Audio/Source") {
            continue;
        }
        let Some(name) = props["node.name"].as_str().filter(|n| !n.is_empty()) else {
            continue;
        };
        // Description first, then the short nick, then the node name — which
        // is unreadable ("alsa_input.pci-0000_00_1f.3-platform-...") but is
        // still better than a blank row.
        let label = ["node.description", "node.nick"]
            .iter()
            .find_map(|key| props[*key].as_str().filter(|v| !v.is_empty()))
            .unwrap_or(name);
        out.push(AudioSource { target: name.to_string(), label: label.to_string() });
    }
    // Two devices can share a description ("Digital Microphone" on a dock and
    // on the lid); dedupe on the target so the selector cannot offer the same
    // node twice, and sort so the list does not reshuffle between mounts.
    out.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| a.target.cmp(&b.target)));
    out.dedup_by(|a, b| a.target == b.target);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from real `pw-dump` output on this laptop, 2026-09-10.
    const DUMP: &str = r#"[
      {"id": 33, "type": "PipeWire:Interface:Node",
       "info": {"props": {"media.class": "Audio/Sink",
                          "node.name": "alsa_output.pci-0000_00_1f.3.HiFi__Speaker__sink",
                          "node.description": "Speakers"}}},
      {"id": 57, "type": "PipeWire:Interface:Node",
       "info": {"props": {"media.class": "Audio/Source",
                          "node.name": "alsa_input.pci-0000_00_1f.3-platform-skl_hda_dsp_generic.HiFi__Mic1__source",
                          "node.description": "Raptor Lake-P/U/H cAVS Digital Microphone",
                          "node.nick": "Digital Microphone",
                          "object.serial": 57}}},
      {"id": 121, "type": "PipeWire:Interface:Node",
       "info": {"props": {"media.class": "Audio/Source",
                          "node.name": "bluez_input.F4:33:B7:59:41:B8",
                          "node.description": "David’s Airpods",
                          "object.serial": 644}}},
      {"id": 42, "type": "PipeWire:Interface:Device",
       "info": {"props": {"device.name": "alsa_card.pci-0000_00_1f.3"}}}
    ]"#;

    #[test]
    fn only_capture_nodes_are_offered() {
        let got = parse(DUMP);
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(got.iter().all(|s| !s.target.contains("output")));
    }

    /// The remembered value has to survive a reconnect, and the serial does
    /// not: the AirPods above are node 121 with serial 644 today and something
    /// else tomorrow.
    #[test]
    fn the_target_is_the_node_name_not_the_serial() {
        let got = parse(DUMP);
        assert!(got.iter().any(|s| s.target == "bluez_input.F4:33:B7:59:41:B8"));
        assert!(got.iter().all(|s| s.target.parse::<u64>().is_err()));
    }

    #[test]
    fn the_label_prefers_the_human_readable_description() {
        let got = parse(DUMP);
        let labels: Vec<&str> = got.iter().map(|s| s.label.as_str()).collect();
        assert!(labels.contains(&"Raptor Lake-P/U/H cAVS Digital Microphone"), "{labels:?}");
        assert!(labels.contains(&"David’s Airpods"), "{labels:?}");
    }

    #[test]
    fn a_source_with_no_description_falls_back_to_nick_then_name() {
        let json = r#"[
          {"info": {"props": {"media.class": "Audio/Source",
                              "node.name": "alsa_input.usb-boundary",
                              "node.nick": "Boundary Mic"}}},
          {"info": {"props": {"media.class": "Audio/Source",
                              "node.name": "alsa_input.nameless"}}}
        ]"#;
        let got = parse(json);
        assert_eq!(got[0].label, "Boundary Mic");
        assert_eq!(got[1].label, "alsa_input.nameless");
    }

    /// Every one of these is a machine that should still be able to record
    /// from the system default.
    #[test]
    fn unusable_output_yields_an_empty_list_not_a_failure() {
        assert!(parse("").is_empty());
        assert!(parse("not json").is_empty());
        assert!(parse("{}").is_empty()); // an object, not the expected array
        assert!(parse("[]").is_empty());
        assert!(parse(r#"[{"info": null}]"#).is_empty());
        assert!(parse(r#"[{"info": {"props": {"media.class": "Audio/Source"}}}]"#).is_empty());
    }

    #[test]
    fn the_same_node_is_never_offered_twice() {
        let json = r#"[
          {"info": {"props": {"media.class": "Audio/Source",
                              "node.name": "mic", "node.description": "Mic"}}},
          {"info": {"props": {"media.class": "Audio/Source",
                              "node.name": "mic", "node.description": "Mic"}}}
        ]"#;
        assert_eq!(parse(json).len(), 1);
    }
}
