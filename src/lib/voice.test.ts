import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { speakableText, speechChunks, useDictating, useDictation } from "@/lib/voice";

class FakeRecognition {
  static last: FakeRecognition | null = null;
  lang = "";
  interimResults = false;
  continuous = false;
  maxAlternatives = 1;
  onresult: (() => void) | null = null;
  onerror: ((event: { error: string }) => void) | null = null;
  onend: (() => void) | null = null;
  start() {
    FakeRecognition.last = this;
  }
  stop() {
    this.onend?.();
  }
  abort() {
    this.onerror?.({ error: "aborted" });
    this.onend?.();
  }
}

describe("useDictating", () => {
  afterEach(() => {
    delete (window as { webkitSpeechRecognition?: unknown }).webkitSpeechRecognition;
  });

  it("is true while any dictation listens, so the wake word can hold", () => {
    (window as { webkitSpeechRecognition?: unknown }).webkitSpeechRecognition = FakeRecognition;
    const dictating = renderHook(() => useDictating());
    const first = renderHook(() => useDictation(() => {}, () => {}));
    const second = renderHook(() => useDictation(() => {}, () => {}));
    expect(dictating.result.current).toBe(false);

    act(() => first.result.current.start());
    const firstRecognition = FakeRecognition.last!;
    act(() => second.result.current.start());
    expect(dictating.result.current).toBe(true);

    act(() => firstRecognition.stop());
    expect(dictating.result.current).toBe(true);
    second.unmount();
    expect(dictating.result.current).toBe(false);
  });
});

describe("speakableText", () => {
  it("drops markup, code blocks and addresses but keeps the words", () => {
    const reply = [
      "## Done",
      "- Opened **#7** for [the login fix](https://github.com/o/r/pull/7)",
      "- Ran `npm test`",
      "```sh",
      "npm run build",
      "```",
      "> Details at https://example.com/run/1",
    ].join("\n");
    expect(speakableText(reply)).toBe(
      ["Done", "Opened #7 for the login fix", "Ran npm test", "Details at"].join("\n"),
    );
  });
});

describe("speechChunks", () => {
  it("keeps short text whole and splits long text at sentence ends within the limit", () => {
    expect(speechChunks("One. Two.")).toEqual(["One. Two."]);
    const sentence = "This sentence is about forty characters. ";
    const chunks = speechChunks(sentence.repeat(10), 100);
    expect(chunks.every((chunk) => chunk.length <= 100)).toBe(true);
    expect(chunks.every((chunk) => chunk.endsWith("characters."))).toBe(true);
    expect(chunks.join(" ")).toBe(sentence.repeat(10).trim());
  });

  it("cuts a sentence longer than the limit", () => {
    const chunks = speechChunks("a".repeat(250), 100);
    expect(chunks.map((chunk) => chunk.length)).toEqual([100, 100, 50]);
  });
});
