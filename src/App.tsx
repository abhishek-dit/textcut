import { useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { cuts, editedLength, isFiller, keepRanges, skip, tokens, type Token, type Word } from "./edit";
import "./App.css";

const MEDIA = ["mp4", "mov", "m4v", "mkv", "webm", "mp3", "wav", "m4a", "aac", "flac", "ogg"];
const fmt = (s: number) => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, "0")}`;

export default function App() {
  const [path, setPath] = useState<string | null>(null);
  const [toks, setToks] = useState<Token[]>([]);
  const [duration, setDuration] = useState(0);
  const [deleted, setDeleted] = useState<Set<number>>(new Set());
  const [history, setHistory] = useState<Set<number>[]>([]);
  const [status, setStatus] = useState("");
  const [now, setNow] = useState(0);
  const media = useRef<HTMLVideoElement>(null);

  const cutList = useMemo(() => cuts(toks, deleted), [toks, deleted]);
  const keep = useMemo(() => keepRanges(toks, deleted, duration), [toks, deleted, duration]);
  const fillers = useMemo(() => toks.flatMap((t, i) => (isFiller(t) && !deleted.has(i) ? [i] : [])), [toks, deleted]);
  const pauses = useMemo(() => toks.flatMap((t, i) => (t.kind === "pause" && !deleted.has(i) ? [i] : [])), [toks, deleted]);

  function edit(next: Set<number>) {
    setHistory((h) => [...h.slice(-99), deleted]);
    setDeleted(next);
  }

  function undo() {
    if (!history.length) return;
    setDeleted(history[history.length - 1]);
    setHistory(history.slice(0, -1));
  }

  async function openFile() {
    const picked = await open({ filters: [{ name: "Video or audio", extensions: MEDIA }] });
    if (typeof picked !== "string") return;
    setPath(picked);
    setToks([]);
    setDeleted(new Set());
    setHistory([]);
    setStatus("Transcribing on this computer…");
    try {
      const t = await invoke<{ words: Word[]; silences: [number, number][]; duration: number }>("transcribe", { path: picked });
      setDuration(t.duration);
      setToks(tokens(t.words, t.duration, t.silences));
      setStatus("");
    } catch (e) {
      setStatus(`Transcription failed: ${e}`);
    }
  }

  async function exportFile() {
    if (!path) return;
    const video = !/\.(mp3|wav|m4a|aac|flac|ogg)$/i.test(path);
    const output = await save({
      defaultPath: path.replace(/\.[^.]+$/, video ? "-edited.mp4" : "-edited.m4a"),
      filters: [{ name: video ? "MP4 video" : "M4A audio", extensions: [video ? "mp4" : "m4a"] }],
    });
    if (!output) return;
    setStatus("Exporting…");
    try {
      await invoke("export", { input: path, keep, output });
      setStatus(`Saved ${output}`);
    } catch (e) {
      setStatus(`Export failed: ${e}`);
    }
  }

  // Delete/Backspace on a text selection toggles those tokens; Cmd/Ctrl+Z undoes; Space plays/pauses.
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if ((e.metaKey || e.ctrlKey) && e.key === "z") {
        e.preventDefault();
        undo();
      } else if (e.key === "Delete" || e.key === "Backspace") {
        const sel = window.getSelection();
        if (!sel || sel.isCollapsed) return;
        const picked = [...document.querySelectorAll<HTMLElement>("[data-i]")]
          .filter((el) => sel.containsNode(el, true))
          .map((el) => Number(el.dataset.i));
        if (!picked.length) return;
        e.preventDefault();
        const next = new Set(deleted);
        const restore = picked.every((i) => deleted.has(i));
        picked.forEach((i) => (restore ? next.delete(i) : next.add(i)));
        edit(next);
        sel.removeAllRanges();
      } else if (e.key === " " && media.current && document.activeElement?.tagName !== "BUTTON") {
        e.preventDefault();
        media.current.paused ? media.current.play() : media.current.pause();
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  // Every frame while playing: jump over cuts (preview = the export) and follow the current word.
  useEffect(() => {
    let raf = 0;
    const tick = () => {
      const m = media.current;
      if (m) {
        const to = !m.paused && skip(m.currentTime, cutList);
        if (to) m.currentTime = to;
        setNow(m.currentTime);
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [cutList]);

  const seek = (t: number) => media.current && (media.current.currentTime = t);

  return (
    <main>
      <header>
        <strong>textcut</strong>
        <button onClick={openFile}>Open video or audio</button>
        {toks.length > 0 && (
          <>
            <button disabled={!fillers.length} onClick={() => edit(new Set([...deleted, ...fillers]))}>
              Remove {fillers.length} filler{fillers.length === 1 ? "" : "s"}
            </button>
            <button disabled={!pauses.length} onClick={() => edit(new Set([...deleted, ...pauses]))}>
              Shorten {pauses.length} pause{pauses.length === 1 ? "" : "s"}
            </button>
            <button disabled={!history.length} onClick={undo}>Undo</button>
            <button onClick={() => edit(new Set())} hidden={!deleted.size}>
              Restore all
            </button>
            <span className="len">
              {fmt(duration)} → {fmt(editedLength(keep))}
            </span>
            <button className="primary" onClick={exportFile}>Export</button>
          </>
        )}
      </header>

      {status && <p className="status">{status}</p>}

      {path ? (
        <section className="editor">
          <video ref={media} src={convertFileSrc(path)} controls />
          <article className="transcript">
            {toks.map((t, i) => {
              const cls = [t.kind, deleted.has(i) && "deleted", now >= t.start && now < t.end && "current"]
                .filter(Boolean).join(" ");
              return (
                <span key={i} data-i={i} className={cls} onClick={() => seek(t.start)}>
                  {t.kind === "pause" ? `⏸ ${t.text}` : t.text}{" "}
                </span>
              );
            })}
          </article>
        </section>
      ) : (
        <section className="empty">
          <h1>Edit video by editing text.</h1>
          <p>Open a recording. It's transcribed on this computer, nothing is uploaded.</p>
          <p>Select words and press <kbd>Delete</kbd> to cut them. <kbd>⌘Z</kbd> undoes. <kbd>Space</kbd> plays.</p>
        </section>
      )}
    </main>
  );
}
