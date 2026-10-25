import { describe, expect, it } from "vitest";
import { parseAppLink } from "@/lib/app-links";

describe("parseAppLink", () => {
  it("opens the chat, listening for voice when the link says so", () => {
    expect(parseAppLink("toodo://chat")).toEqual({ voice: false });
    expect(parseAppLink("toodo://chat/")).toEqual({ voice: false });
    expect(parseAppLink("toodo://chat/voice")).toEqual({ voice: true });
    expect(parseAppLink("TOODO://chat/voice?from=shortcut")).toEqual({ voice: true });
  });

  it("ignores links it does not know", () => {
    for (const url of ["toodo://tasks", "toodo://chat/voices", "https://chat/voice", "toodo://chat.example/voice", ""]) {
      expect(parseAppLink(url)).toBeNull();
    }
  });
});
