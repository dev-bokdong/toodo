import { describe, expect, it } from "vitest";

import { markdownPreview } from "./markdown";

describe("markdownPreview", () => {
  it("drops block syntax and joins the lines", () => {
    expect(markdownPreview("## Heading\n\n- [x] done\n1. next\n> quoted\n\n```ts\ncode\n```\n---")).toBe(
      "Heading done next quoted code",
    );
  });

  it("keeps the text of inline syntax", () => {
    expect(
      markdownPreview("Use **bold**, *em*, `code`, ~~old~~, [docs](https://example.com) and ![logo](a.png)"),
    ).toBe("Use bold, em, code, old, docs and logo");
  });

  it("leaves underscores inside words and table cells readable", () => {
    expect(markdownPreview("| a | b |\n| --- | --- |\n| task_id | 2 * 3 |")).toBe("a b task_id 2 * 3");
  });

  it("keeps unpaired markers and link targets with parentheses intact", () => {
    expect(markdownPreview("[bad](javascript:alert(1)) sets window.__pwned and 2*3")).toBe(
      "bad sets window.__pwned and 2*3",
    );
  });

  it("is empty for an empty description", () => {
    expect(markdownPreview("")).toBe("");
    expect(markdownPreview("\n```\n```\n")).toBe("");
  });
});
