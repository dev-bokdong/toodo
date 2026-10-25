import { describe, expect, it } from "vitest";
import { speakableText, speechChunks } from "@/lib/voice";

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
