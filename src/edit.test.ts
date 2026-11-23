// npm test
import { test } from "node:test";
import assert from "node:assert/strict";
import { captionCues, cuts, editedCues, isFiller, keepRanges, skip, toEdited, tokens } from "./edit.ts";

test("captions break at punctuation, cuts and length; times map onto the cut video", () => {
  const w = "one two three four five six seven. eight nine".split(" ")
    .map((text, i) => ({ text, start: i, end: i + 0.5 }));
  const t = tokens(w, 10);
  assert.deepEqual(captionCues(t, new Set()).map((c) => c.text), ["one two three", "four five six", "seven.", "eight nine"]);
  assert.equal(captionCues(t, new Set([1]))[0].text, "one"); // a cut ends the line
  const keep = [{ start: 0, end: 2 }, { start: 5, end: 10 }];
  assert.equal(toEdited(6, keep), 3);
  assert.equal(toEdited(3, keep), 2); // inside a cut: snaps to where the cut is
  assert.deepEqual(editedCues([{ start: 5.5, end: 6, text: "x" }], keep), [{ text: "x", start: 2.5, end: 3 }]);
});

const words = [
  { start: 0.0, end: 0.4, text: "Hello" },
  { start: 0.5, end: 0.7, text: "um," },
  { start: 0.8, end: 3.0, text: "world" }, // whisper stretched this word across the pause
  { start: 3.0, end: 3.4, text: "again" },
];
const silences: [number, number][] = [[1.2, 3.0], [3.4, 4.0], [0.4, 0.45]];
const toks = tokens(words, 4, silences);

test("silences become padded pause tokens and trim the words that swallowed them", () => {
  assert.deepEqual(toks.map((t) => t.kind), ["word", "word", "word", "pause", "word", "pause"]);
  assert.equal(toks[2].end, 1.2);
  assert.deepEqual([toks[3].start, toks[3].end].map((x) => +x.toFixed(2)), [1.35, 2.85]);
});

test("fillers are found through punctuation", () => {
  assert.deepEqual(toks.map((t, i) => (isFiller(t) ? i : -1)).filter((i) => i >= 0), [1]);
});

test("adjacent deletions merge into one cut; the rest is kept", () => {
  const del = new Set([1, 2, 3]);
  assert.deepEqual(cuts(toks, del), [{ start: 0.5, end: toks[3].end }]);
  assert.deepEqual(keepRanges(toks, del, 4), [{ start: 0, end: 0.5 }, { start: toks[3].end, end: 4 }]);
});

test("nothing deleted keeps everything; deleting all keeps nothing", () => {
  assert.deepEqual(keepRanges(toks, new Set(), 4), [{ start: 0, end: 4 }]);
  const all = new Set(toks.map((_, i) => i));
  assert.deepEqual(keepRanges(tokens([{ start: 0, end: 4, text: "x" }], 4), new Set([0]), 4), []);
  assert.ok(keepRanges(toks, all, 4).every((r) => r.end - r.start < 0.2)); // only the pause pads remain
});

test("playback jumps over cuts", () => {
  const cs = cuts(toks, new Set([1]));
  assert.equal(skip(0.6, cs), 0.7);
  assert.equal(skip(0.2, cs), null);
});
