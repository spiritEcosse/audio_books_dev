//! Local image upscaling for video export. Upscayl (Real-ESRGAN, the illustration model
//! `digital-art-4x`) enlarges a scene image, then ffmpeg fits it exactly to 4K
//! (cover-scale + centre crop, so a non-16:9 image is cropped rather than squeezed).
//!
//! The result lives next to the source image as `<image name>-4k.png`, so no database column is
//! needed: a version exists when its file does, and a regenerated image (new versioned name) starts without one.
//! The Upscayl binary and model are bundled from `resources/upscayl` (see scripts/fetch-upscayl.sh).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::claude;
use crate::error::AppError;

const MODEL_NAME: &str = "digital-art-4x";
const UPSCAYL_DIR: &str = "upscayl";
const UPSCAYL_BIN: &str = "upscayl-bin";
const MODELS_DIR: &str = "models";
const OUTPUT_EXTENSION: &str = "png";
const FETCH_HINT: &str = "run scripts/fetch-upscayl.sh and rebuild the app";
const ERROR_TAIL_CHARS: usize = 400;

/// 3840x2160: headroom for zooms and pans, and YouTube re-encodes 4K uploads better.
pub const SIZE: (u32, u32) = (3840, 2160);
/// Names the enlarged copy and the exported video.
pub const LABEL: &str = "4k";
/// Upscayl enlarges the source this many times; 1024px wide images reach 4K at 4x.
const SCALE: u32 = 4;

pub fn upscaled_path(image: &Path) -> PathBuf {
    let stem = image.file_stem().unwrap_or_default().to_string_lossy();
    image.with_file_name(format!("{stem}-{LABEL}.{OUTPUT_EXTENSION}"))
}

/// The enlarged copy of `image`, if it exists on disk.
pub fn existing_upscaled(image: &Path) -> Option<String> {
    let path = upscaled_path(image);
    path.is_file().then(|| path.display().to_string())
}

fn upscayl_args(input: &Path, output: &Path, scale: u32, models: &Path) -> Vec<OsString> {
    vec![
        "-i".into(),
        input.into(),
        "-o".into(),
        output.into(),
        "-s".into(),
        scale.to_string().into(),
        "-n".into(),
        MODEL_NAME.into(),
        "-m".into(),
        models.into(),
        "-f".into(),
        OUTPUT_EXTENSION.into(),
    ]
}

fn fit_filter(width: u32, height: u32) -> String {
    format!(
        "scale={width}:{height}:force_original_aspect_ratio=increase:flags=lanczos,crop={width}:{height}"
    )
}

fn ffmpeg_args(input: &Path, output: &Path) -> Vec<OsString> {
    let (width, height) = SIZE;
    vec![
        "-y".into(),
        "-v".into(),
        "error".into(),
        "-i".into(),
        input.into(),
        "-vf".into(),
        fit_filter(width, height).into(),
        output.into(),
    ]
}

/// The bundled Upscayl files and the user's ffmpeg.
#[derive(Debug, Clone)]
pub struct Tools {
    upscayl: PathBuf,
    models: PathBuf,
    ffmpeg: PathBuf,
}

/// `roots` are the directories that may contain `upscayl/` (the app's resources, the source tree in dev).
pub fn find_tools(roots: &[PathBuf]) -> Result<Tools, AppError> {
    let dir = roots
        .iter()
        .map(|root| root.join(UPSCAYL_DIR))
        .find(|dir| dir.join(UPSCAYL_BIN).is_file())
        .ok_or_else(|| AppError::new(format!("Upscayl is not installed: {FETCH_HINT}")))?;
    let models = dir.join(MODELS_DIR);
    if !models.join(format!("{MODEL_NAME}.param")).is_file() {
        return Err(AppError::new(format!(
            "the {MODEL_NAME} model is missing: {FETCH_HINT}"
        )));
    }
    let ffmpeg = claude::find_in_login_shell("ffmpeg")?
        .ok_or_else(|| AppError::new("ffmpeg not found. Install it, e.g. brew install ffmpeg"))?;
    Ok(Tools {
        upscayl: dir.join(UPSCAYL_BIN),
        models,
        ffmpeg,
    })
}

fn run_tool(program: &Path, args: &[OsString], name: &str) -> Result<(), AppError> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| AppError::new(format!("cannot run {name}: {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    let log = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let tail: String = log
        .chars()
        .rev()
        .take(ERROR_TAIL_CHARS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    Err(AppError::new(format!(
        "{name} failed ({}): {tail}",
        output.status
    )))
}

fn enlarge(image: &Path, tools: &Tools, raw: &Path, part: &Path) -> Result<(), AppError> {
    run_tool(
        &tools.upscayl,
        &upscayl_args(image, raw, SCALE, &tools.models),
        "Upscayl",
    )?;
    run_tool(&tools.ffmpeg, &ffmpeg_args(raw, part), "ffmpeg")
}

/// Enlarges one image to 4K and returns the new file. Blocking: about 25 seconds per image.
pub fn upscale(image: &Path, tools: &Tools) -> Result<PathBuf, AppError> {
    if !image.is_file() {
        return Err(AppError::new(format!(
            "image not found: {}",
            image.display()
        )));
    }
    let target = upscaled_path(image);
    let raw = target.with_extension("raw.png");
    let part = target.with_extension("part.png");
    let result = enlarge(image, tools, &raw, &part).and_then(|()| {
        std::fs::rename(&part, &target)
            .map_err(|e| AppError::new(format!("cannot save {}: {e}", target.display())))
    });
    let _ = std::fs::remove_file(&raw);
    let _ = std::fs::remove_file(&part);
    result.map(|()| target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("audiobooks-upscale-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn upscaled_path_sits_next_to_the_image_with_the_label() {
        assert_eq!(
            upscaled_path(Path::new("/media/7/scene-0-image-v1.webp")),
            Path::new("/media/7/scene-0-image-v1-4k.png")
        );
    }

    #[test]
    fn fit_filter_covers_and_crops_to_the_exact_size() {
        assert_eq!(
            fit_filter(3840, 2160),
            "scale=3840:2160:force_original_aspect_ratio=increase:flags=lanczos,crop=3840:2160"
        );
    }

    #[test]
    fn upscayl_args_name_the_model_scale_and_models_folder() {
        let args = upscayl_args(
            Path::new("in.webp"),
            Path::new("out.png"),
            4,
            Path::new("/m"),
        );
        let text: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            text,
            [
                "-i",
                "in.webp",
                "-o",
                "out.png",
                "-s",
                "4",
                "-n",
                "digital-art-4x",
                "-m",
                "/m",
                "-f",
                "png"
            ]
        );
    }

    #[test]
    fn ffmpeg_args_overwrite_quietly_and_end_with_the_output() {
        let args = ffmpeg_args(Path::new("raw.png"), Path::new("out.png"));
        let text: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(&text[..5], ["-y", "-v", "error", "-i", "raw.png"]);
        assert!(text[6].contains("crop=3840:2160"));
        assert_eq!(text.last().unwrap(), "out.png");
    }

    #[test]
    fn existing_upscaled_is_found_only_once_the_file_exists() {
        let dir = temp_dir("versions");
        let image = dir.join("scene-0-image-v1.webp");
        assert_eq!(existing_upscaled(&image), None);
        std::fs::write(upscaled_path(&image), b"x").unwrap();
        assert!(
            existing_upscaled(&image)
                .unwrap()
                .ends_with("scene-0-image-v1-4k.png")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn find_tools_reports_missing_upscayl_with_the_fix() {
        let dir = temp_dir("no-tools");
        let err = find_tools(std::slice::from_ref(&dir))
            .unwrap_err()
            .to_string();
        assert!(err.contains("fetch-upscayl.sh"), "{err}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn upscale_rejects_a_missing_image() {
        let tools = Tools {
            upscayl: "u".into(),
            models: "m".into(),
            ffmpeg: "f".into(),
        };
        let err = upscale(Path::new("/nonexistent/scene.webp"), &tools).unwrap_err();
        assert!(err.to_string().contains("image not found"));
    }

    /// Needs the bundled Upscayl (scripts/fetch-upscayl.sh) and ffmpeg: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn upscale_produces_the_exact_size_for_16_9_and_square_sources() {
        let roots = [PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/resources"
        ))];
        let tools = find_tools(&roots).unwrap();
        let dir = temp_dir("real");
        for (name, size) in [("wide", "512x288"), ("square", "256x256")] {
            let image = dir.join(format!("{name}.png"));
            let made = Command::new(&tools.ffmpeg)
                .args([
                    "-y",
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("testsrc=size={size}:rate=1"),
                    "-frames:v",
                    "1",
                ])
                .arg(&image)
                .status()
                .unwrap();
            assert!(made.success());
            let out = upscale(&image, &tools).unwrap();
            let probe = Command::new("ffprobe")
                .args([
                    "-v",
                    "error",
                    "-show_entries",
                    "stream=width,height",
                    "-of",
                    "csv=p=0",
                ])
                .arg(&out)
                .output()
                .unwrap();
            let (w, h) = SIZE;
            assert_eq!(
                String::from_utf8_lossy(&probe.stdout).trim(),
                format!("{w},{h}"),
                "{name}"
            );
            assert!(!dir.join(format!("{name}-4k.raw.png")).exists());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
