//! Video export: one clip per scene (a smooth zoom or pan over the scene image), joined with 1 s fades, the
//! narration laid over at one loudness (see `loudness`). Each clip is rendered on its own and cached by a hash of its inputs, so editing one
//! scene only re-renders that scene; a final ffmpeg run joins the clips and mixes the audio.
//!
//! Timing: a scene's slot is LEAD + narration + TAIL seconds; its clip is TRANSITION longer so it overlaps the
//! next clip during the fade. The result is `video-1080p.mp4` / `video-4k.mp4` in the story's media folder, with
//! English subtitles beside it in an `.srt` file of the same name (see `subtitles`).

use std::collections::hash_map::DefaultHasher;
use std::ffi::OsString;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

use crate::claude;
use crate::error::AppError;
use crate::loudness::{self, Measurement};
use crate::subtitles;
use crate::upscale::{self, Resolution};

/// The message of the error a cancelled export ends with; see `was_cancelled`.
const CANCELLED_MESSAGE: &str = "export cancelled";

/// True when the export stopped because the user cancelled it, not because something broke.
pub fn was_cancelled(error: &AppError) -> bool {
    error.to_string() == CANCELLED_MESSAGE
}

pub const FPS: u32 = 30;
const TRANSITION_SECS: f64 = 1.0;
const LEAD_SECS: f64 = 0.5;
const TAIL_SECS: f64 = 0.5;
/// Zoom in / out over a clip: 15 % larger at the closest point.
const ZOOM_TOTAL: f64 = 0.15;
/// Pans run at a fixed 12 % zoom so there is room to travel.
const PAN_ZOOM: f64 = 1.12;
/// The final join: slow preset, high quality.
const CRF: &str = "18";
/// Scene clips are intermediates that the join re-encodes, so they use a fast preset (measured: SSIM 0.987 vs 0.989 after the join,
/// 1.6x faster than medium); crf 16 keeps the loss within noise.
const CLIP_PRESET: &str = "veryfast";
const CLIP_CRF: &str = "16";
const AUDIO_BITRATE: &str = "192k";
const CLIP_DIR: &str = "export";
/// The final join renders far faster per frame than a smooth zoom clip, so it counts for less of the bar.
const ASSEMBLY_WEIGHT: f64 = 0.4;
const ERROR_TAIL_CHARS: usize = 400;

/// How far an export is, sent to the UI while it runs.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Progress {
    pub percent: f64,
    /// "clip" while scene clips render, "join" for the final run.
    pub stage: &'static str,
    /// The 1-based scene being rendered (the last one during the join).
    pub scene: usize,
    pub scenes: usize,
}

fn percent(done: f64, total: f64) -> f64 {
    if total <= 0.0 {
        100.0
    } else {
        (done / total * 100.0).clamp(0.0, 100.0)
    }
}

/// A `frame=N` line of ffmpeg's `-progress` output.
fn parse_frame(line: &str) -> Option<u64> {
    line.strip_prefix("frame=")?.trim().parse().ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Motion {
    ZoomIn,
    PanRight,
    ZoomOut,
    PanLeft,
}

/// Motions alternate by scene so a long video does not feel repetitive.
pub fn motion_for(position: i64) -> Motion {
    match position.rem_euclid(4) {
        0 => Motion::ZoomIn,
        1 => Motion::PanRight,
        2 => Motion::ZoomOut,
        _ => Motion::PanLeft,
    }
}

/// Where each scene sits on the final timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Length of each scene's clip (slot + the overlap with the next clip).
    pub clip_secs: Vec<f64>,
    /// When each clip starts on the final timeline.
    pub clip_starts: Vec<f64>,
    /// When each narration starts on the final timeline.
    pub voice_starts: Vec<f64>,
}

pub fn plan(narration_secs: &[f64]) -> Plan {
    let mut plan = Plan {
        clip_secs: Vec::new(),
        clip_starts: Vec::new(),
        voice_starts: Vec::new(),
    };
    let mut start = 0.0;
    for narration in narration_secs {
        let slot = LEAD_SECS + narration + TAIL_SECS;
        plan.clip_secs.push(slot + TRANSITION_SECS);
        plan.clip_starts.push(start);
        plan.voice_starts.push(start + LEAD_SECS);
        start += slot;
    }
    plan
}

fn frame_count(secs: f64) -> u32 {
    (secs * f64::from(FPS)).ceil() as u32
}

fn sizes(resolution: Resolution) -> ((u32, u32), (u32, u32)) {
    let work = match resolution {
        Resolution::Hd => (2560, 1440),
        Resolution::Uhd => (3840, 2160),
    };
    (resolution.size(), work)
}

/// The sub-pixel smooth zoom/pan: `perspective` cuts a window with fractional coordinates (unlike zoompan, which
/// rounds to whole pixels and jitters). The image is decoded once and looped by the filter.
pub fn motion_filter(motion: Motion, frames: u32, resolution: Resolution) -> String {
    let ((out_w, out_h), (work_w, work_h)) = sizes(resolution);
    let last = frames.saturating_sub(1).max(1);
    let p = format!("(on/{last})");
    let eased = format!("({p}*{p}*(3-2*{p}))");
    let zoom = match motion {
        Motion::ZoomIn => format!("(1+{ZOOM_TOTAL}*{eased})"),
        Motion::ZoomOut => format!("(1+{ZOOM_TOTAL}*(1-{eased}))"),
        Motion::PanRight | Motion::PanLeft => PAN_ZOOM.to_string(),
    };
    let (w, h) = (format!("(W/{zoom})"), format!("(H/{zoom})"));
    let x = match motion {
        Motion::PanRight => format!("((W-{w})*{eased})"),
        Motion::PanLeft => format!("((W-{w})*(1-{eased}))"),
        Motion::ZoomIn | Motion::ZoomOut => format!("((W-{w})/2)"),
    };
    let y = format!("((H-{h})/2)");
    format!(
        "scale={work_w}:{work_h}:flags=lanczos,loop=loop=-1:size=1:start=0,fps={FPS},\
         perspective=x0='{x}':y0='{y}':x1='{x}+{w}':y1='{y}':x2='{x}':y2='{y}+{h}':x3='{x}+{w}':y3='{y}+{h}':\
         eval=frame:interpolation=linear,scale={out_w}:{out_h}:flags=lanczos,format=yuv420p"
    )
}

fn clip_args(
    image: &Path,
    motion: Motion,
    secs: f64,
    resolution: Resolution,
    output: &Path,
) -> Vec<OsString> {
    vec![
        "-y".into(),
        "-v".into(),
        "error".into(),
        "-i".into(),
        image.into(),
        "-t".into(),
        format!("{secs:.3}").into(),
        "-r".into(),
        FPS.to_string().into(),
        "-vf".into(),
        motion_filter(motion, frame_count(secs), resolution).into(),
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        CLIP_PRESET.into(),
        "-crf".into(),
        CLIP_CRF.into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        output.into(),
    ]
}

/// Joins the clips with fades and mixes each narration in at its start time, evened out by its measurement
/// (`loudness` has one entry per narration).
pub fn assemble_args(
    clips: &[PathBuf],
    audios: &[PathBuf],
    loudness: &[Option<Measurement>],
    plan: &Plan,
    output: &Path,
) -> Vec<OsString> {
    let count = clips.len();
    let mut args: Vec<OsString> = vec!["-y".into(), "-v".into(), "error".into()];
    for input in clips.iter().chain(audios) {
        args.push("-i".into());
        args.push(input.into());
    }
    let mut graph: Vec<String> = Vec::new();
    let mut video = "0:v".to_owned();
    for (index, start) in plan.clip_starts.iter().enumerate().take(count).skip(1) {
        let label = format!("x{index}");
        graph.push(format!(
            "[{video}][{index}:v]xfade=transition=fade:duration={TRANSITION_SECS}:offset={start:.3}[{label}]"
        ));
        video = label;
    }
    let delayed: Vec<String> = (0..count)
        .map(|index| {
            let millis = (plan.voice_starts[index] * 1000.0).round() as u64;
            graph.push(format!(
                "[{}:a]{},adelay={millis}:all=1[a{index}]",
                count + index,
                loudness::filter(loudness[index].as_ref())
            ));
            format!("[a{index}]")
        })
        .collect();
    // After `loudnorm`, `amix` can stamp a frame with no time, which the muxer rejects; the mix is continuous, so
    // its times are counted afresh from the samples.
    graph.push(format!(
        "{}amix=inputs={count}:normalize=0:duration=longest,asetpts=N/SR/TB[aout]",
        delayed.concat()
    ));
    args.extend([
        "-filter_complex".into(),
        graph.join(";").into(),
        "-map".into(),
    ]);
    args.push(
        if count == 1 {
            video
        } else {
            format!("[{video}]")
        }
        .into(),
    );
    args.extend(["-map".into(), "[aout]".into()]);
    for part in [
        "-c:v",
        "libx264",
        "-preset",
        "medium",
        "-crf",
        CRF,
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-b:a",
        AUDIO_BITRATE,
        "-movflags",
        "+faststart",
    ] {
        args.push(part.into());
    }
    args.push(output.into());
    args
}

/// The biggest enlarged copy of a scene image that exists, else the original.
pub fn best_image(original: &Path) -> PathBuf {
    [Resolution::Uhd, Resolution::Hd]
        .into_iter()
        .map(|resolution| upscale::upscaled_path(original, resolution))
        .find(|path| path.is_file())
        .unwrap_or_else(|| original.to_path_buf())
}

#[derive(Debug, Clone, PartialEq)]
pub struct SceneSource {
    pub position: i64,
    pub image: PathBuf,
    pub audio: PathBuf,
    /// The text the narration was generated from; `None` when that is no longer known.
    pub narration: Option<String>,
}

/// A finished video next to the scene media.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct VideoFile {
    pub label: &'static str,
    pub file_path: String,
}

pub fn video_path(media_dir: &Path, resolution: Resolution) -> PathBuf {
    media_dir.join(format!("video-{}.mp4", resolution.label()))
}

pub fn subtitles_path(video: &Path) -> PathBuf {
    video.with_extension("srt")
}

/// Writes the subtitles of a finished video. When a scene's voiced text is unknown no file is written, and one left
/// by an earlier export is removed: wrong or stale subtitles are worse than none.
fn write_subtitles(
    video: &Path,
    scenes: &[SceneSource],
    narration_secs: &[f64],
    plan: &Plan,
) -> Result<(), AppError> {
    let path = subtitles_path(video);
    let narrations: Option<Vec<subtitles::Narration>> = scenes
        .iter()
        .zip(narration_secs)
        .zip(&plan.voice_starts)
        .map(|((scene, &secs), &start)| {
            let text = scene.narration.as_deref()?;
            Some(subtitles::Narration { text, start, secs })
        })
        .collect();
    let content = narrations
        .map(|all| subtitles::srt(&all))
        .unwrap_or_default();
    let written = if content.is_empty() {
        match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    } else {
        std::fs::write(&path, content)
    };
    written.map_err(|e| AppError::new(format!("cannot save {}: {e}", path.display())))
}

pub fn existing_videos(media_dir: &Path) -> Vec<VideoFile> {
    Resolution::ALL
        .into_iter()
        .filter_map(|resolution| {
            let path = video_path(media_dir, resolution);
            path.is_file().then(|| VideoFile {
                label: resolution.label(),
                file_path: path.display().to_string(),
            })
        })
        .collect()
}

fn clip_cache_path(
    dir: &Path,
    scene: &SceneSource,
    motion: Motion,
    secs: f64,
    resolution: Resolution,
) -> PathBuf {
    let stamp = std::fs::metadata(&scene.image)
        .ok()
        .map(|meta| (meta.len(), meta.modified().ok()))
        .unwrap_or_default();
    let mut hasher = DefaultHasher::new();
    (
        &scene.image,
        stamp,
        motion,
        frame_count(secs),
        resolution.label(),
        CLIP_PRESET,
        CLIP_CRF,
    )
        .hash(&mut hasher);
    dir.join(format!(
        "clip-{}-{:016x}.mp4",
        scene.position,
        hasher.finish()
    ))
}

fn run_tool(program: &Path, args: &[OsString], name: &str) -> Result<String, AppError> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| AppError::new(format!("cannot run {name}: {e}")))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    Err(AppError::new(format!(
        "{name} failed ({}): {}",
        output.status,
        log_tail(&String::from_utf8_lossy(&output.stderr))
    )))
}

/// The end of a tool's log, where it says what went wrong.
pub(crate) fn log_tail(log: &str) -> String {
    let log = log.trim();
    let skip = log.chars().count().saturating_sub(ERROR_TAIL_CHARS);
    log.chars().skip(skip).collect()
}

fn find_tool(name: &str) -> Result<PathBuf, AppError> {
    claude::find_in_login_shell(name)?.ok_or_else(|| {
        AppError::new(format!(
            "{name} not found. Install ffmpeg, e.g. brew install ffmpeg"
        ))
    })
}

fn narration_seconds(ffprobe: &Path, audio: &Path) -> Result<f64, AppError> {
    let args: Vec<OsString> = [
        "-v",
        "error",
        "-show_entries",
        "format=duration",
        "-of",
        "csv=p=0",
    ]
    .map(OsString::from)
    .into();
    let mut args = args;
    args.push(audio.into());
    let text = run_tool(ffprobe, &args, "ffprobe")?;
    text.trim()
        .parse::<f64>()
        .ok()
        .filter(|secs| secs.is_finite() && *secs > 0.0)
        .ok_or_else(|| AppError::new(format!("cannot read the length of {}", audio.display())))
}

/// Runs ffmpeg and reports the number of the frame it is on, as ffmpeg prints it about twice a second.
/// Once `cancel` is set (checked on each report) ffmpeg is killed and the run ends with the cancelled error.
fn run_ffmpeg(
    ffmpeg: &Path,
    args: &[OsString],
    cancel: &AtomicBool,
    mut on_frame: impl FnMut(u64),
) -> Result<(), AppError> {
    if cancel.load(Ordering::SeqCst) {
        return Err(AppError::new(CANCELLED_MESSAGE));
    }
    let mut child = Command::new(ffmpeg)
        .args(["-progress", "pipe:1", "-nostats"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AppError::new(format!("cannot run ffmpeg: {e}")))?;
    let stderr = child.stderr.take();
    // Drained on its own thread so a chatty ffmpeg can never block on a full pipe.
    let log = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut stream) = stderr {
            let _ = stream.read_to_string(&mut text);
        }
        text
    });
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(frame) = parse_frame(&line) {
                on_frame(frame);
            }
            if cancel.load(Ordering::SeqCst) {
                let _ = child.kill();
                let _ = child.wait();
                let _ = log.join();
                return Err(AppError::new(CANCELLED_MESSAGE));
            }
        }
    }
    let status = child
        .wait()
        .map_err(|e| AppError::new(format!("ffmpeg stopped: {e}")))?;
    if status.success() {
        return Ok(());
    }
    let tail = log_tail(&log.join().unwrap_or_default());
    Err(AppError::new(format!("ffmpeg failed ({status}): {tail}")))
}

/// Writes through a `.part` file so a failed or interrupted run never leaves a broken video behind.
fn render_atomically(
    ffmpeg: &Path,
    build: impl Fn(&Path) -> Vec<OsString>,
    target: &Path,
    cancel: &AtomicBool,
    on_frame: impl FnMut(u64),
) -> Result<(), AppError> {
    let mut part = target.as_os_str().to_owned();
    part.push(".part.mp4");
    let part = PathBuf::from(part);
    let result = run_ffmpeg(ffmpeg, &build(&part), cancel, on_frame).and_then(|()| {
        std::fs::rename(&part, target)
            .map_err(|e| AppError::new(format!("cannot save {}: {e}", target.display())))
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

/// Renders the whole story. Blocking and long: roughly 2-5 s of render per second of video for new clips.
/// `on_progress` is called about twice a second while ffmpeg works. Setting `cancel` stops the render within about a
/// second; finished clips stay cached, so the next export resumes from them.
pub fn export(
    scenes: &[SceneSource],
    resolution: Resolution,
    media_dir: &Path,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<PathBuf, AppError> {
    if scenes.is_empty() {
        return Err(AppError::new("the story has no scenes to export"));
    }
    let ffmpeg = find_tool("ffmpeg")?;
    let ffprobe = find_tool("ffprobe")?;
    let narration: Vec<f64> = scenes
        .iter()
        .map(|scene| narration_seconds(&ffprobe, &scene.audio))
        .collect::<Result<_, _>>()?;
    let plan = plan(&narration);
    let clip_dir = media_dir.join(CLIP_DIR);
    std::fs::create_dir_all(&clip_dir)
        .map_err(|e| AppError::new(format!("cannot create {}: {e}", clip_dir.display())))?;

    let clips: Vec<PathBuf> = scenes
        .iter()
        .enumerate()
        .map(|(index, scene)| {
            clip_cache_path(
                &clip_dir,
                scene,
                motion_for(scene.position),
                plan.clip_secs[index],
                resolution,
            )
        })
        .collect();
    let to_render: Vec<usize> = (0..scenes.len())
        .filter(|index| !clips[*index].is_file())
        .collect();
    let clip_frames = |index: usize| f64::from(frame_count(plan.clip_secs[index]));
    let last = scenes.len() - 1;
    let join_frames = f64::from(frame_count(plan.clip_starts[last] + plan.clip_secs[last]));
    let total = to_render
        .iter()
        .map(|index| clip_frames(*index))
        .sum::<f64>()
        + join_frames * ASSEMBLY_WEIGHT;
    let scene_count = scenes.len();
    let mut report = |stage: &'static str, scene: usize, done: f64| {
        on_progress(Progress {
            percent: percent(done, total),
            stage,
            scene,
            scenes: scene_count,
        });
    };

    let mut done = 0.0;
    for &index in &to_render {
        let (motion, secs) = (motion_for(scenes[index].position), plan.clip_secs[index]);
        let image = best_image(&scenes[index].image);
        report("clip", index + 1, done);
        render_atomically(
            &ffmpeg,
            |out| clip_args(&image, motion, secs, resolution, out),
            &clips[index],
            cancel,
            |frame| report("clip", index + 1, done + frame as f64),
        )?;
        done += clip_frames(index);
    }
    let audios: Vec<PathBuf> = scenes.iter().map(|scene| scene.audio.clone()).collect();
    let target = video_path(media_dir, resolution);
    report("join", scene_count, done);
    // Under a second per scene, so the cancel flag is only checked between scenes.
    let measured: Vec<Option<Measurement>> = audios
        .iter()
        .map(|audio| {
            if cancel.load(Ordering::SeqCst) {
                return Err(AppError::new(CANCELLED_MESSAGE));
            }
            loudness::measure(&ffmpeg, audio)
        })
        .collect::<Result<_, _>>()?;
    render_atomically(
        &ffmpeg,
        |out| assemble_args(&clips, &audios, &measured, &plan, out),
        &target,
        cancel,
        |frame| report("join", scene_count, done + frame as f64 * ASSEMBLY_WEIGHT),
    )?;
    write_subtitles(&target, scenes, &narration, &plan)?;
    report("join", scene_count, total);
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("audiobooks-video-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn plan_gives_each_scene_a_slot_with_lead_tail_and_overlap() {
        let plan = plan(&[10.0, 20.0]);
        assert_eq!(plan.clip_secs, [12.0, 22.0]);
        assert_eq!(plan.clip_starts, [0.0, 11.0]);
        assert_eq!(plan.voice_starts, [0.5, 11.5]);
    }

    #[test]
    fn motions_cycle_through_four_by_scene_position() {
        let motions: Vec<Motion> = (0..5).map(motion_for).collect();
        assert_eq!(
            motions,
            [
                Motion::ZoomIn,
                Motion::PanRight,
                Motion::ZoomOut,
                Motion::PanLeft,
                Motion::ZoomIn
            ]
        );
        assert_eq!(motion_for(-1), Motion::PanLeft);
    }

    #[test]
    fn motion_filter_uses_fractional_perspective_and_the_exact_output_size() {
        let filter = motion_filter(Motion::ZoomIn, 300, Resolution::Hd);
        assert!(filter.starts_with(
            "scale=2560:1440:flags=lanczos,loop=loop=-1:size=1:start=0,fps=30,perspective="
        ));
        assert!(filter.contains("on/299"));
        assert!(filter.contains("eval=frame"));
        assert!(filter.ends_with("scale=1920:1080:flags=lanczos,format=yuv420p"));
        assert!(
            motion_filter(Motion::PanLeft, 90, Resolution::Uhd)
                .ends_with("scale=3840:2160:flags=lanczos,format=yuv420p")
        );
    }

    #[test]
    fn zoom_out_runs_the_zoom_backwards_and_pans_move_sideways() {
        assert!(motion_filter(Motion::ZoomOut, 100, Resolution::Hd).contains("(1+0.15*(1-"));
        let right = motion_filter(Motion::PanRight, 100, Resolution::Hd);
        let left = motion_filter(Motion::PanLeft, 100, Resolution::Hd);
        assert!(right.contains("1.12"));
        assert_ne!(right, left);
    }

    #[test]
    fn clip_args_cap_the_length_and_end_with_the_output() {
        let args = text(&clip_args(
            Path::new("a.png"),
            Motion::ZoomIn,
            12.0,
            Resolution::Hd,
            Path::new("c.mp4"),
        ));
        assert_eq!(&args[3..7], ["-i", "a.png", "-t", "12.000"]);
        assert_eq!(args.last().unwrap(), "c.mp4");
        let encoder = args.iter().position(|a| a == "-preset").unwrap();
        assert_eq!(
            &args[encoder..encoder + 4],
            ["-preset", "veryfast", "-crf", "16"]
        );
    }

    #[test]
    fn assemble_chains_fades_at_each_scene_start_and_delays_each_voice() {
        let clips: Vec<PathBuf> = ["c0.mp4", "c1.mp4", "c2.mp4"].map(PathBuf::from).into();
        let audios: Vec<PathBuf> = ["a0.mp3", "a1.mp3", "a2.mp3"].map(PathBuf::from).into();
        let args = text(&assemble_args(
            &clips,
            &audios,
            &[None, None, None],
            &plan(&[10.0, 20.0, 5.0]),
            Path::new("out.mp4"),
        ));
        let graph = &args[args.iter().position(|a| a == "-filter_complex").unwrap() + 1];
        assert!(
            graph.contains("[0:v][1:v]xfade=transition=fade:duration=1:offset=11.000[x1]"),
            "{graph}"
        );
        assert!(
            graph.contains("[x1][2:v]xfade=transition=fade:duration=1:offset=32.000[x2]"),
            "{graph}"
        );
        let even = loudness::filter(None);
        assert!(
            graph.contains(&format!("[3:a]{even},adelay=500:all=1[a0]")),
            "{graph}"
        );
        assert!(
            graph.contains(&format!("[5:a]{even},adelay=32500:all=1[a2]")),
            "{graph}"
        );
        assert!(
            graph.ends_with(
                "[a0][a1][a2]amix=inputs=3:normalize=0:duration=longest,asetpts=N/SR/TB[aout]"
            ),
            "{graph}"
        );
        assert!(args.contains(&"[x2]".to_owned()));
        assert_eq!(args.last().unwrap(), "out.mp4");
    }

    #[test]
    fn a_single_scene_skips_the_fade_chain() {
        let args = text(&assemble_args(
            &[PathBuf::from("c.mp4")],
            &[PathBuf::from("a.mp3")],
            &[None],
            &plan(&[4.0]),
            Path::new("o.mp4"),
        ));
        let graph = &args[args.iter().position(|a| a == "-filter_complex").unwrap() + 1];
        assert!(!graph.contains("xfade"));
        assert!(args.contains(&"0:v".to_owned()));
    }

    #[test]
    fn best_image_prefers_the_biggest_enlarged_copy_and_falls_back_to_the_original() {
        let dir = temp_dir("best");
        let original = dir.join("scene-0-image-v1.webp");
        assert_eq!(best_image(&original), original);
        std::fs::write(upscale::upscaled_path(&original, Resolution::Hd), b"x").unwrap();
        assert_eq!(
            best_image(&original),
            upscale::upscaled_path(&original, Resolution::Hd)
        );
        std::fs::write(upscale::upscaled_path(&original, Resolution::Uhd), b"x").unwrap();
        assert_eq!(
            best_image(&original),
            upscale::upscaled_path(&original, Resolution::Uhd)
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn existing_videos_lists_only_finished_files() {
        let dir = temp_dir("videos");
        assert!(existing_videos(&dir).is_empty());
        std::fs::write(video_path(&dir, Resolution::Hd), b"x").unwrap();
        let videos = existing_videos(&dir);
        assert_eq!(videos.len(), 1);
        assert_eq!(videos[0].label, "1080p");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn clip_cache_name_changes_with_the_inputs_but_not_between_runs() {
        let dir = temp_dir("cache");
        let image = dir.join("scene.png");
        std::fs::write(&image, b"abc").unwrap();
        let scene = SceneSource {
            position: 2,
            image,
            audio: dir.join("a.mp3"),
            narration: None,
        };
        let base = clip_cache_path(&dir, &scene, Motion::ZoomOut, 12.0, Resolution::Hd);
        assert_eq!(
            base,
            clip_cache_path(&dir, &scene, Motion::ZoomOut, 12.0, Resolution::Hd)
        );
        assert_ne!(
            base,
            clip_cache_path(&dir, &scene, Motion::ZoomOut, 13.0, Resolution::Hd)
        );
        assert_ne!(
            base,
            clip_cache_path(&dir, &scene, Motion::ZoomOut, 12.0, Resolution::Uhd)
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn percent_is_clamped_and_an_empty_job_counts_as_done() {
        assert_eq!(percent(0.0, 200.0), 0.0);
        assert_eq!(percent(50.0, 200.0), 25.0);
        assert_eq!(percent(300.0, 200.0), 100.0);
        assert_eq!(percent(-5.0, 200.0), 0.0);
        assert_eq!(percent(0.0, 0.0), 100.0);
    }

    #[test]
    fn parse_frame_reads_only_frame_lines_of_the_progress_output() {
        assert_eq!(parse_frame("frame=123"), Some(123));
        assert_eq!(parse_frame("frame=  7 "), Some(7));
        assert_eq!(parse_frame("fps=29.97"), None);
        assert_eq!(parse_frame("frame=N/A"), None);
        assert_eq!(parse_frame("progress=continue"), None);
    }

    #[test]
    fn subtitles_follow_the_voice_starts_and_are_removed_when_a_voiced_text_is_unknown() {
        let dir = temp_dir("srt");
        let video = video_path(&dir, Resolution::Uhd);
        let scene = |position: i64, narration: Option<&str>| SceneSource {
            position,
            image: dir.join("i.png"),
            audio: dir.join("a.mp3"),
            narration: narration.map(str::to_owned),
        };
        let secs = [2.0, 3.0];
        let known = [scene(0, Some("One.")), scene(1, Some("Two."))];

        write_subtitles(&video, &known, &secs, &plan(&secs)).unwrap();
        let srt = std::fs::read_to_string(dir.join("video-4k.srt")).unwrap();
        assert!(srt.contains("00:00:00,500 --> 00:00:02,500\nOne."), "{srt}");
        assert!(srt.contains("00:00:03,500 --> 00:00:06,500\nTwo."), "{srt}");

        let unknown = [scene(0, Some("One.")), scene(1, None)];
        write_subtitles(&video, &unknown, &secs, &plan(&secs)).unwrap();
        assert!(!subtitles_path(&video).exists());
        write_subtitles(&video, &unknown, &secs, &plan(&secs)).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn export_rejects_an_empty_story() {
        let err = export(
            &[],
            Resolution::Hd,
            Path::new("/tmp"),
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap_err();
        assert!(err.to_string().contains("no scenes"));
    }

    #[cfg(unix)]
    #[test]
    fn cancelling_kills_ffmpeg_and_leaves_no_part_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("cancel");
        let fake = dir.join("fake-ffmpeg");
        std::fs::write(&fake, "#!/bin/sh\nfor i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do echo frame=$i; sleep 1; done\n")
            .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let target = dir.join("out.mp4");
        let cancel = AtomicBool::new(false);
        let started = std::time::Instant::now();

        let result = render_atomically(
            &fake,
            |part| {
                std::fs::write(part, b"half a video").unwrap();
                Vec::new()
            },
            &target,
            &cancel,
            |frame| {
                if frame >= 2 {
                    cancel.store(true, Ordering::SeqCst);
                }
            },
        );

        assert!(was_cancelled(&result.unwrap_err()));
        assert!(
            started.elapsed().as_secs() < 10,
            "the fake render takes 20 s if it is not killed"
        );
        assert!(!target.exists());
        assert!(!dir.join("out.mp4.part.mp4").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_cancel_set_before_the_start_stops_without_running_ffmpeg() {
        let cancel = AtomicBool::new(true);
        let error =
            run_ffmpeg(Path::new("/definitely/not/ffmpeg"), &[], &cancel, |_| {}).unwrap_err();
        assert!(was_cancelled(&error), "{error}");
    }

    #[test]
    fn other_errors_are_not_mistaken_for_a_cancel() {
        assert!(!was_cancelled(&AppError::new(
            "ffmpeg failed (exit status: 1)"
        )));
    }

    /// The mean level in dB of a part of a file's sound.
    fn mean_volume(ffmpeg: &Path, file: &Path, start: &str, seconds: &str) -> f64 {
        let output = Command::new(ffmpeg)
            .args(["-nostdin", "-ss", start, "-t", seconds, "-i"])
            .arg(file)
            .args(["-vn", "-af", "volumedetect", "-f", "null", "-"])
            .output()
            .unwrap();
        let log = String::from_utf8_lossy(&output.stderr);
        let (_, rest) = log.split_once("mean_volume: ").unwrap();
        rest.split_whitespace().next().unwrap().parse().unwrap()
    }

    /// Needs ffmpeg. These lengths made `amix` stamp a broken time on a frame after `loudnorm`, which the AAC muxer
    /// rejects; the two short scenes of the export test below never showed it.
    #[test]
    #[ignore]
    fn the_join_survives_several_normalised_narrations() {
        let ffmpeg = find_tool("ffmpeg").unwrap();
        let dir = temp_dir("join");
        let lengths = [7.3, 8.1, 6.7];
        let plan = plan(&lengths);
        let make = |input: String, output: &Path| {
            let mut args: Vec<OsString> = ["-y", "-v", "error", "-f", "lavfi", "-i", &input]
                .map(OsString::from)
                .into();
            args.push(output.into());
            run_tool(&ffmpeg, &args, "ffmpeg").unwrap();
        };
        let (mut clips, mut audios) = (Vec::new(), Vec::new());
        for (index, seconds) in lengths.iter().enumerate() {
            let clip = dir.join(format!("clip-{index}.mp4"));
            let secs = plan.clip_secs[index];
            make(
                format!("color=size=64x36:rate={FPS}:duration={secs}"),
                &clip,
            );
            clips.push(clip);
            let audio = dir.join(format!("scene-{index}.mp3"));
            let (pitch, gain) = (300 + index * 100, index * 6);
            make(
                format!("sine=frequency={pitch}:duration={seconds},volume=-{gain}dB"),
                &audio,
            );
            audios.push(audio);
        }
        let measured: Vec<Option<Measurement>> = audios
            .iter()
            .map(|audio| loudness::measure(&ffmpeg, audio).unwrap())
            .collect();
        let video = dir.join("out.mp4");
        let args = assemble_args(&clips, &audios, &measured, &plan, &video);
        run_tool(&ffmpeg, &args, "ffmpeg").unwrap();
        assert!(video.is_file());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Needs ffmpeg and ffprobe: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn export_joins_two_scenes_into_a_1080p_video_with_audio() {
        let ffmpeg = find_tool("ffmpeg").unwrap();
        let ffprobe = find_tool("ffprobe").unwrap();
        let dir = temp_dir("real");
        let mut scenes = Vec::new();
        for (position, seconds) in [(0_i64, 2), (1, 3)] {
            let image = dir.join(format!("scene-{position}.png"));
            let audio = dir.join(format!("scene-{position}.mp3"));
            for (input, output) in [
                ("testsrc=size=640x360:rate=1".to_string(), &image),
                (
                    format!(
                        "sine=frequency={}:duration={seconds},volume={}dB",
                        300 + position * 200,
                        position * -12
                    ),
                    &audio,
                ),
            ] {
                let mut args: Vec<OsString> = ["-y", "-v", "error", "-f", "lavfi", "-i", &input]
                    .map(OsString::from)
                    .into();
                if output.extension().is_some_and(|ext| ext == "png") {
                    args.extend(["-frames:v".into(), "1".into()]);
                }
                args.push(output.into());
                run_tool(&ffmpeg, &args, "ffmpeg").unwrap();
            }
            scenes.push(SceneSource {
                position,
                image,
                audio,
                narration: Some(format!("[softly] Scene {position}.")),
            });
        }
        let mut reports: Vec<Progress> = Vec::new();
        let video = export(
            &scenes,
            Resolution::Hd,
            &dir,
            &AtomicBool::new(false),
            &mut |progress| reports.push(progress),
        )
        .unwrap();
        assert!(reports.len() > 3, "{reports:?}");
        assert!(
            reports
                .windows(2)
                .all(|pair| pair[1].percent >= pair[0].percent),
            "{reports:?}"
        );
        assert_eq!(reports.last().unwrap().percent, 100.0);
        assert!(reports.iter().any(|p| p.stage == "clip" && p.scene == 2));
        assert_eq!(reports.last().unwrap().stage, "join");
        let probe: Vec<OsString> = [
            "-v",
            "error",
            "-show_entries",
            "stream=codec_type,width,height",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
        ]
        .map(OsString::from)
        .into();
        let mut args = probe;
        args.push((&video).into());
        let info = run_tool(&ffprobe, &args, "ffprobe").unwrap();
        assert!(info.contains("video,1920,1080"), "{info}");
        assert!(info.contains("audio"), "{info}");
        // Both narrations, 12 dB apart at the source, sit at the target in the stereo track.
        for (start, seconds) in [("0.5", "2"), ("3.5", "3")] {
            let level = mean_volume(&ffmpeg, &video, start, seconds);
            let first = mean_volume(&ffmpeg, &video, "0.5", "2");
            assert!((level - first).abs() < 1.5, "{level} vs {first}");
        }
        // slots 3 s + 4 s plus the 1 s overlap of the last clip = 8 s
        let duration: f64 = info.lines().last().unwrap().trim().parse().unwrap();
        assert!((duration - 8.0).abs() < 0.5, "{duration}");
        assert!(video_path(&dir, Resolution::Hd).is_file());
        let srt = std::fs::read_to_string(subtitles_path(&video)).unwrap();
        assert!(
            srt.contains("Scene 0.") && srt.contains("Scene 1."),
            "{srt}"
        );
        assert!(!srt.contains("softly"), "{srt}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
