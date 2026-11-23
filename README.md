# textcut

**Edit video by editing text.** An open-source, local-first alternative to Descript.

Open a recording, and textcut transcribes it on your computer. Delete words in the transcript and the video cuts
itself. One click removes every "um", "uh" and long pause. Nothing is uploaded, and there's no account or
subscription.

<!-- demo.gif: select a sentence, press Delete, the video skips it, Export -->

## What it does

- **Transcribes locally** with [whisper.cpp](https://github.com/ggml-org/whisper.cpp): every word gets its own
  timestamp, in 99 languages
- **Edit the text, the video follows:** select words, press <kbd>Delete</kbd>. Playback already skips the cuts, so
  what you hear is what you export
- **Remove fillers** (um, uh, erm, hmm…) and **shorten pauses** in one click. Pauses come from the audio itself, not
  from guessed word timings
- **Undo** everything (<kbd>⌘Z</kbd>), restore any cut word by deleting it again
- **Exports** MP4 (H.264/AAC) or M4A for audio-only recordings, with ffmpeg

## Install (from source, for now)

You need [Rust](https://rustup.rs), Node 20+, ffmpeg 7+ and whisper.cpp.

```bash
# macOS
brew install ffmpeg whisper-cpp
git clone https://github.com/abhishek-dit/textcut && cd textcut
npm install
scripts/get-model.sh          # 1.6 GB, best quality; `scripts/get-model.sh small` for a 466 MB one
npm run tauri dev
```

Linux: install `ffmpeg` and build [whisper.cpp](https://github.com/ggml-org/whisper.cpp) so `whisper-cli` is on your
PATH. Set `TEXTCUT_MODEL=/path/to/ggml-model.bin` to use a model from somewhere else.

## How it works

| Step | Tool |
|---|---|
| Audio → 16 kHz WAV, silence detection | ffmpeg (`silencedetect`) |
| Word-level transcript | whisper.cpp (`-ml 1 -sow`), prompted to keep fillers it normally tidies away |
| The edit | a set of deleted words/pauses → cut list → kept ranges ([src/edit.ts](src/edit.ts)) |
| Export | ffmpeg `trim`/`atrim` + `concat`, filter graph from a file so 500 cuts are fine |

## Tests

```bash
npm test                                                     # the edit model
TEXTCUT_SAMPLE=clip.mp4 cargo test --manifest-path src-tauri/Cargo.toml -- --ignored   # real transcribe → cut → export
```

## Roadmap

- [ ] Save/open projects (`.textcut` file next to the video)
- [ ] Burned-in captions with styles, including correct Indic shaping (Kannada, Tamil, Malayalam, Hindi…)
- [ ] 9:16 export for Shorts/Reels
- [ ] Bundled whisper.cpp + ffmpeg, signed installers for macOS, Windows, Linux
- [ ] Noise removal ("studio sound") with an open model
- [ ] Multi-track (two speakers, screen + camera)
- [ ] Fix a mis-spoken word with an open TTS voice

## License

MIT
