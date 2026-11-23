//! textcut backend: transcription (whisper.cpp) and export (ffmpeg), both run as local processes.
//! Nothing leaves the machine.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Serialize)]
struct Word {
    start: f64, // seconds
    end: f64,
    text: String,
}

#[derive(Serialize)]
struct Transcript {
    words: Vec<Word>,
    silences: Vec<(f64, f64)>,
    duration: f64,
}

#[derive(Deserialize)]
struct Range {
    start: f64,
    end: f64,
}

/// GUI apps on macOS don't get the shell PATH, so look in the usual install places too.
fn tool(name: &str) -> Result<PathBuf, String> {
    let from_path = std::env::var("PATH").unwrap_or_default();
    let dirs = from_path.split(':').map(PathBuf::from)
        .chain(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].map(PathBuf::from));
    for d in dirs {
        let p = d.join(name);
        if p.is_file() {
            return Ok(p);
        }
    }
    Err(format!("{name} not found. Install it (macOS: brew install {})",
                if name == "whisper-cli" { "whisper-cpp" } else { "ffmpeg" }))
}

/// TEXTCUT_MODEL, else the biggest ggml model in ~/.textcut/models (scripts/get-model.sh puts one there).
fn model() -> Result<PathBuf, String> {
    if let Ok(m) = std::env::var("TEXTCUT_MODEL") {
        return Ok(m.into());
    }
    let dir = PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".textcut/models");
    std::fs::read_dir(&dir).ok().into_iter().flatten().flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "bin"))
        .max_by_key(|p| p.metadata().map(|m| m.len()).unwrap_or(0))
        .ok_or_else(|| format!("No whisper model in {}. Run scripts/get-model.sh", dir.display()))
}

fn run(cmd: &mut Command) -> Result<String, String> {
    let out = cmd.output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err.lines().rev().take(3).collect::<Vec<_>>().join(" | "));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("textcut-{}-{name}", std::process::id()))
}

fn duration(input: &Path) -> Result<f64, String> {
    let out = run(Command::new(tool("ffprobe")?)
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"]).arg(input))?;
    out.trim().parse().map_err(|_| format!("can't read duration of {}", input.display()))
}

fn has_video(input: &Path) -> Result<bool, String> {
    let out = run(Command::new(tool("ffprobe")?)
        .args(["-v", "error", "-select_streams", "v", "-show_entries", "stream=codec_type", "-of", "csv=p=0"])
        .arg(input))?;
    Ok(out.contains("video"))
}

/// Silent stretches from the audio itself. Whisper's word timestamps stretch across pauses, so they can't be
/// trusted to show them.
fn silences(wav: &Path) -> Result<Vec<(f64, f64)>, String> {
    let out = Command::new(tool("ffmpeg")?).args(["-hide_banner", "-nostats", "-i"]).arg(wav)
        .args(["-af", "silencedetect=noise=-35dB:d=0.4", "-f", "null", "-"])
        .output().map_err(|e| e.to_string())?;
    let log = String::from_utf8_lossy(&out.stderr);
    let num = |line: &str, key: &str| line.split(key).nth(1)?.split_whitespace().next()?.parse::<f64>().ok();
    let mut out = vec![];
    let mut start = None;
    for line in log.lines() {
        if let Some(s) = num(line, "silence_start: ") {
            start = Some(s.max(0.0));
        } else if let (Some(s), Some(e)) = (start, num(line, "silence_end: ")) {
            out.push((s, e));
            start = None;
        }
    }
    Ok(out)
}

fn transcribe_blocking(input: PathBuf) -> Result<Transcript, String> {
    let wav = tmp("audio.wav");
    run(Command::new(tool("ffmpeg")?).args(["-y", "-loglevel", "error", "-i"]).arg(&input)
        .args(["-ar", "16000", "-ac", "1", "-c:a", "pcm_s16le"]).arg(&wav))?;
    let silences = silences(&wav)?;
    let base = tmp("words");
    // -ml 1 -sow: one segment per word, so every word carries its own timestamps
    run(Command::new(tool("whisper-cli")?).arg("-m").arg(model()?).arg("-f").arg(&wav)
        .args(["-l", "auto", "-ml", "1", "-sow", "-oj", "-np"])
        // Whisper tidies speech by dropping fillers; a disfluent prompt makes it write them down
        .args(["--prompt", "Umm, so, uh, let me think, like, hmm... Okay, uh, here's what I, um, mean."])
        .arg("-of").arg(&base))?;
    let json_path = base.with_extension("json");
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&json_path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&wav);
    let _ = std::fs::remove_file(&json_path);
    let words = json["transcription"].as_array().into_iter().flatten()
        .filter_map(|s| {
            let text = s["text"].as_str()?.trim().to_string();
            let start = s["offsets"]["from"].as_f64()? / 1000.0;
            let end = s["offsets"]["to"].as_f64()? / 1000.0;
            (!text.is_empty()).then_some(Word { start, end, text })
        })
        .collect();
    Ok(Transcript { words, silences, duration: duration(&input)? })
}

/// Cut the kept ranges out of the input and join them. The filter graph goes in a file: an edit with hundreds
/// of cuts is too long for a command line.
fn export_blocking(input: PathBuf, keep: Vec<Range>, output: PathBuf) -> Result<(), String> {
    if keep.is_empty() {
        return Err("nothing left to export".into());
    }
    let video = has_video(&input)?;
    let mut graph = String::new();
    let mut joins = String::new();
    for (i, r) in keep.iter().enumerate() {
        if video {
            graph += &format!("[0:v]trim={:.3}:{:.3},setpts=PTS-STARTPTS[v{i}];", r.start, r.end);
            joins += &format!("[v{i}]");
        }
        graph += &format!("[0:a]atrim={:.3}:{:.3},asetpts=PTS-STARTPTS[a{i}];", r.start, r.end);
        joins += &format!("[a{i}]");
    }
    graph += &format!("{joins}concat=n={}:v={}:a=1{}[a]", keep.len(), video as u8, if video { "[v]" } else { "" });
    let script = tmp("graph.txt");
    std::fs::write(&script, graph).map_err(|e| e.to_string())?;
    let mut cmd = Command::new(tool("ffmpeg")?);
    cmd.args(["-y", "-loglevel", "error", "-i"]).arg(&input).arg("-/filter_complex").arg(&script);
    if video {
        cmd.args(["-map", "[v]", "-c:v", "libx264", "-preset", "veryfast", "-crf", "18", "-pix_fmt", "yuv420p"]);
    }
    cmd.args(["-map", "[a]", "-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart"]).arg(&output);
    let res = run(&mut cmd);
    let _ = std::fs::remove_file(&script);
    res.map(|_| ())
}

#[tauri::command]
async fn transcribe(path: String) -> Result<Transcript, String> {
    tauri::async_runtime::spawn_blocking(move || transcribe_blocking(path.into())).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn export(input: String, keep: Vec<Range>, output: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || export_blocking(input.into(), keep, output.into()))
        .await.map_err(|e| e.to_string())?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run_app() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![transcribe, export])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real whisper + ffmpeg run. Needs a model (scripts/get-model.sh) and a recording with a pause in it:
    ///   TEXTCUT_SAMPLE=clip.mp4 cargo test -- --ignored
    #[test]
    #[ignore]
    fn transcribe_cut_export() {
        let input = PathBuf::from(std::env::var("TEXTCUT_SAMPLE").expect("set TEXTCUT_SAMPLE"));
        let t = transcribe_blocking(input.clone()).unwrap();
        assert!(t.words.len() > 3, "too few words: {}", t.words.len());
        assert!(t.silences.iter().any(|(a, b)| b - a > 1.0), "no pause found: {:?}", t.silences);
        let (a, b) = *t.silences.iter().max_by(|x, y| (x.1 - x.0).total_cmp(&(y.1 - y.0))).unwrap();
        let out = tmp("export.mp4");
        export_blocking(input, vec![Range { start: 0.0, end: a }, Range { start: b, end: t.duration }], out.clone()).unwrap();
        let expected = t.duration - (b - a);
        let got = duration(&out).unwrap();
        assert!((got - expected).abs() < 0.15, "exported {got:.2}s, expected {expected:.2}s");
        assert!(has_video(&out).unwrap());
        let _ = std::fs::remove_file(out);
    }
}
