//! Narration loudness: every scene is generated on its own, so the export brings them all to one level with ffmpeg's
//! two-pass `loudnorm` (measure, then apply). It is measured and applied on a stereo copy, because that is how a mono
//! voice is heard on two speakers. The source files are not changed.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use serde::Deserialize;

use crate::error::AppError;
use crate::video;

/// Integrated loudness, true peak ceiling and loudness range of the exported narration.
const TARGET: &str = "I=-16:TP=-1.5:LRA=11";
const SAMPLE_RATE: u32 = 48_000;
const STEREO: &str = "aformat=channel_layouts=stereo";

/// What the first pass found, kept as ffmpeg printed it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Measurement {
    input_i: String,
    input_tp: String,
    input_lra: String,
    input_thresh: String,
    target_offset: String,
}

fn measure_args(audio: &Path) -> Vec<OsString> {
    vec![
        "-nostdin".into(),
        "-nostats".into(),
        "-hide_banner".into(),
        "-i".into(),
        audio.into(),
        "-af".into(),
        format!("{STEREO},loudnorm={TARGET}:print_format=json").into(),
        "-f".into(),
        "null".into(),
        "-".into(),
    ]
}

/// Reads the JSON block `loudnorm` prints last. `None` means a silent file, which cannot be normalised.
fn parse_measurement(log: &str) -> Result<Option<Measurement>, AppError> {
    let unreadable = || AppError::new("cannot read the loudness report of ffmpeg");
    let json = log.rfind('{').map(|start| &log[start..]);
    let json = json
        .and_then(|rest| rest.find('}').map(|end| &rest[..=end]))
        .ok_or_else(unreadable)?;
    let measured: Measurement = serde_json::from_str(json).map_err(|_| unreadable())?;
    let audible = measured.input_i.parse::<f64>().is_ok_and(f64::is_finite);
    Ok(audible.then_some(measured))
}

/// The filter chain for one narration; every chain ends in the same stereo format and sample rate.
pub fn filter(measured: Option<&Measurement>) -> String {
    let normalise = measured.map_or_else(String::new, |m| {
        format!(
            "loudnorm={TARGET}:measured_I={}:measured_TP={}:measured_LRA={}:measured_thresh={}:offset={}:linear=true,",
            m.input_i, m.input_tp, m.input_lra, m.input_thresh, m.target_offset
        )
    });
    // loudnorm works at 192 kHz inside, so the rate is set back after it.
    format!("{STEREO},{normalise}aresample={SAMPLE_RATE}")
}

/// The first pass over one narration.
pub fn measure(ffmpeg: &Path, audio: &Path) -> Result<Option<Measurement>, AppError> {
    let output = Command::new(ffmpeg)
        .args(measure_args(audio))
        .output()
        .map_err(|e| AppError::new(format!("cannot run ffmpeg: {e}")))?;
    let log = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(AppError::new(format!(
            "cannot measure the loudness of {} ({}): {}",
            audio.display(),
            output.status,
            video::log_tail(&log)
        )));
    }
    parse_measurement(&log)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "[Parsed_loudnorm_1 @ 0x1] \n{\n\t\"input_i\" : \"-20.90\",\n\t\"input_tp\" : \"-2.10\",\n\t\
        \"input_lra\" : \"7.60\",\n\t\"input_thresh\" : \"-31.20\",\n\t\"output_i\" : \"-16.00\",\n\t\
        \"target_offset\" : \"0.10\"\n}\n";

    fn measured() -> Measurement {
        parse_measurement(LOG).unwrap().unwrap()
    }

    #[test]
    fn first_pass_asks_for_a_stereo_json_report_and_writes_nothing() {
        let args: Vec<String> = measure_args(Path::new("a.mp3"))
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&format!("{STEREO},loudnorm={TARGET}:print_format=json")));
        assert_eq!(&args[args.len() - 3..], ["-f", "null", "-"]);
    }

    #[test]
    fn parses_the_report_at_the_end_of_the_log() {
        assert_eq!(measured().input_i, "-20.90");
        assert_eq!(measured().target_offset, "0.10");
    }

    #[test]
    fn a_silent_file_has_no_measurement_and_a_missing_report_is_an_error() {
        let silent = LOG.replace("-20.90", "-inf");
        assert_eq!(parse_measurement(&silent).unwrap(), None);
        assert!(parse_measurement("no report here").is_err());
        assert!(parse_measurement("{ \"input_i\": \"-20\" }").is_err());
    }

    #[test]
    fn second_pass_feeds_the_measurement_back_and_fixes_the_format() {
        assert_eq!(
            filter(Some(&measured())),
            "aformat=channel_layouts=stereo,loudnorm=I=-16:TP=-1.5:LRA=11:measured_I=-20.90:measured_TP=-2.10:\
             measured_LRA=7.60:measured_thresh=-31.20:offset=0.10:linear=true,aresample=48000"
        );
        assert_eq!(
            filter(None),
            "aformat=channel_layouts=stereo,aresample=48000"
        );
    }
}
