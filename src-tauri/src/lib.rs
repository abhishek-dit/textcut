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

/// One caption, already in the edited timeline.
#[derive(Deserialize)]
struct Cue {
    start: f64,
    end: f64,
    text: String,
}

/// GUI apps on macOS don't get the shell PATH, so look in the usual install places too.
fn tool(name: &str) -> Result<PathBuf, String> {
    let from_path = std::env::var("PATH").unwrap_or_default();
    // Homebrew's ffmpeg-full is keg-only; it has libass (captions), the default ffmpeg doesn't
    let dirs = ["/opt/homebrew/opt/ffmpeg-full/bin", "/usr/local/opt/ffmpeg-full/bin"].map(PathBuf::from).into_iter()
        .chain(from_path.split(':').map(PathBuf::from))
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

fn video_size(input: &Path) -> Result<(u32, u32), String> {
    let out = run(Command::new(tool("ffprobe")?)
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "csv=p=0"])
        .arg(input))?;
    let mut it = out.trim().split(',').filter_map(|x| x.parse().ok());
    Ok((it.next().ok_or("no video size")?, it.next().ok_or("no video size")?))
}

fn stamp(t: f64, sep: char, cs: bool) -> String {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    let (h, m, s, f) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
    if cs { format!("{h}:{m:02}:{s:02}{sep}{:02}", f / 10) } else { format!("{h:02}:{m:02}:{s:02}{sep}{f:03}") }
}

fn srt(cues: &[Cue]) -> String {
    cues.iter().enumerate()
        .map(|(i, c)| format!("{}\n{} --> {}\n{}\n", i + 1, stamp(c.start, ',', false), stamp(c.end, ',', false), c.text))
        .collect::<Vec<_>>().join("\n")
}

/// Bold white text with a dark outline in the lower third, sized from the frame (works for 16:9 and 9:16).
fn ass(cues: &[Cue], (w, h): (u32, u32)) -> String {
    let size = (h.min(w * 16 / 9) as f64 * 0.055).round() as u32;
    let margin = h / 8;
    let mut out = format!("[Script Info]\nScriptType: v4.00+\nPlayResX: {w}\nPlayResY: {h}\nWrapStyle: 0\n\
        ScaledBorderAndShadow: yes\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, OutlineColour, \
        BackColour, Bold, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
        Style: Cap,Helvetica Neue,{size},&H00FFFFFF,&H00000000,&H80000000,-1,{},1,2,{m},{m},{margin},1\n\n\
        [Events]\nFormat: Layer, Start, End, Style, Text\n", size / 12 + 2, m = w / 12);
    for c in cues {
        let text = c.text.replace('{', "(").replace('}', ")").replace('\n', " ");
        out += &format!("Dialogue: 0,{},{},Cap,{text}\n", stamp(c.start, '.', true), stamp(c.end, '.', true));
    }
    out
}

/// Cut the kept ranges out of the input and join them. The filter graph goes in a file: an edit with hundreds
/// of cuts is too long for a command line. Captions (if any) are written next to the output as .srt, and burned
/// in with libass (shaping=complex, so Indic scripts join correctly) when burn is set.
fn export_blocking(input: PathBuf, keep: Vec<Range>, output: PathBuf, captions: Vec<Cue>, burn: bool) -> Result<(), String> {
    if keep.is_empty() {
        return Err("nothing left to export".into());
    }
    let video = has_video(&input)?;
    if !captions.is_empty() {
        std::fs::write(output.with_extension("srt"), srt(&captions)).map_err(|e| e.to_string())?;
    }
    let burn = burn && video && !captions.is_empty();
    let ass_file = tmp("captions.ass");
    if burn {
        let filters = run(Command::new(tool("ffmpeg")?).args(["-hide_banner", "-filters"]))?;
        if !filters.lines().any(|l| l.split_whitespace().nth(1) == Some("ass")) {
            return Err("Burned-in captions need ffmpeg with libass (macOS: brew install ffmpeg-full). \
                        The .srt file was saved.".into());
        }
        std::fs::write(&ass_file, ass(&captions, video_size(&input)?)).map_err(|e| e.to_string())?;
    }
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
    let vout = if burn { "[vc]" } else if video { "[v]" } else { "" };
    graph += &format!("{joins}concat=n={}:v={}:a=1{vout}[a]", keep.len(), video as u8);
    if burn {  // filter-graph escaping: \ and : and ' are special inside the filename
        let f = ass_file.display().to_string().replace('\\', "\\\\").replace(':', "\\:").replace('\'', "\\'");
        graph += &format!(";[vc]ass=filename='{f}':shaping=complex[v]");
    }
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
    let _ = std::fs::remove_file(&ass_file);
    res.map(|_| ())
}

/// Projects are a small JSON file next to the media (talk.mp4 -> talk.mp4.textcut). Written to a temp file
/// first, so a crash mid-save never leaves a half-written project.
#[tauri::command]
fn load_project(path: String) -> Result<Option<String>, String> {
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
fn save_project(path: String, data: String) -> Result<(), String> {
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, data).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| e.to_string())
}

#[tauri::command]
async fn transcribe(path: String) -> Result<Transcript, String> {
    tauri::async_runtime::spawn_blocking(move || transcribe_blocking(path.into())).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn export(input: String, keep: Vec<Range>, output: String, captions: Vec<Cue>, burn: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || export_blocking(input.into(), keep, output.into(), captions, burn))
        .await.map_err(|e| e.to_string())?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run_app() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![transcribe, export, load_project, save_project])
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
        let keep = vec![Range { start: 0.0, end: a }, Range { start: b, end: t.duration }];
        let cues = vec![Cue { start: 0.2, end: 1.5, text: "ಕನ್ನಡ ಅಕ್ಷರ தமிழ் മലയാളം".into() }];
        export_blocking(input, keep, out.clone(), cues, true).unwrap();
        assert!(out.with_extension("srt").exists());
        if let Ok(frame) = std::env::var("TEXTCUT_FRAME") {  // look at the burned-in caption shaping by eye
            run(Command::new(tool("ffmpeg").unwrap()).args(["-y", "-loglevel", "error", "-ss", "1", "-i"]).arg(&out)
                .args(["-frames:v", "1", &frame])).unwrap();
        }
        let expected = t.duration - (b - a);
        let got = duration(&out).unwrap();
        assert!((got - expected).abs() < 0.15, "exported {got:.2}s, expected {expected:.2}s");
        assert!(has_video(&out).unwrap());
        let _ = std::fs::remove_file(out.with_extension("srt"));
        let _ = std::fs::remove_file(out);
    }

    #[test]
    fn caption_files() {
        let cues = [Cue { start: 3661.5, end: 3662.25, text: "hi {x}".into() }];
        assert_eq!(srt(&cues), "1\n01:01:01,500 --> 01:01:02,250\nhi {x}\n");
        let a = ass(&cues, (1080, 1920));
        assert!(a.contains("PlayResY: 1920") && a.contains("Dialogue: 0,1:01:01.50,1:01:02.25,Cap,hi (x)"), "{a}");
    }
}
