// The whole editing model: a transcript is a list of tokens (words + pauses); an edit is the set of deleted
// tokens. Everything else — playback skipping, export — is derived from that, so undo is just a copy of the set.

export type Word = { start: number; end: number; text: string };
export type Token = Word & { kind: "word" | "pause" };
export type Range = { start: number; end: number };

export const MIN_PAUSE = 0.5; // silences at least this long become deletable pause tokens
const PAD = 0.15; // silence left on each side when a pause is deleted, so cuts don't sound clipped
const MIN_KEEP = 0.05; // kept slivers shorter than this are dropped (they'd only click)

const FILLERS = new Set(["um", "umm", "uh", "uhh", "uhm", "er", "erm", "ah", "hmm", "hum", "mm", "mhm"]);

/** Words + pause tokens. Pauses come from the audio's silences (whisper stretches words across pauses), and
 * words are trimmed to them so deleting a word doesn't also delete the silence it swallowed. */
export function tokens(words: Word[], duration: number, silences: [number, number][] = []): Token[] {
  const ws = words.map((w) => {
    let { start, end } = w;
    for (const [a, b] of silences) {
      if (start < a && end > a) end = a; // word runs into a silence
      else if (start >= a && start < b && end > b) start = b; // word starts inside a silence
    }
    return { kind: "word" as const, start, end, text: w.text };
  });
  const ps = silences
    .filter(([a, b]) => b - a >= MIN_PAUSE)
    .map(([a, b]) => ({ kind: "pause" as const, start: a + PAD, end: Math.min(b, duration) - PAD, text: `${(b - a).toFixed(1)}s` }));
  return [...ws, ...ps].sort((x, y) => x.start - y.start);
}

/** Consecutive deleted tokens become one cut, so the gaps between them go too. */
export function cuts(toks: Token[], deleted: Set<number>): Range[] {
  const out: Range[] = [];
  for (let i = 0; i < toks.length; i++) {
    if (!deleted.has(i)) continue;
    let j = i;
    while (deleted.has(j + 1)) j++;
    out.push({ start: toks[i].start, end: toks[j].end });
    i = j;
  }
  return out;
}

export function keepRanges(toks: Token[], deleted: Set<number>, duration: number): Range[] {
  const out: Range[] = [];
  let t = 0;
  for (const c of cuts(toks, deleted)) {
    if (c.start - t >= MIN_KEEP) out.push({ start: t, end: c.start });
    t = Math.max(t, c.end);
  }
  if (duration - t >= MIN_KEEP) out.push({ start: t, end: duration });
  return out;
}

/** During playback: if t is inside a cut, where to jump to. */
export function skip(t: number, cs: Range[]): number | null {
  const c = cs.find((c) => t >= c.start && t < c.end - 0.01);
  return c ? c.end : null;
}

export const isFiller = (tok: Token) =>
  tok.kind === "word" && FILLERS.has(tok.text.toLowerCase().replace(/[^\p{L}]/gu, ""));

export const editedLength = (ranges: Range[]) => ranges.reduce((s, r) => s + r.end - r.start, 0);
