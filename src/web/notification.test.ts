import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { sendNotification } from "@/web/notification";
import { pushActive } from "@/web/push";

vi.mock("@/web/push", () => ({ pushActive: vi.fn() }));

describe("sendNotification", () => {
  const shown = vi.fn();

  beforeEach(() => {
    shown.mockReset();
    vi.stubGlobal(
      "Notification",
      class {
        constructor(title: string, options?: NotificationOptions) {
          shown(title, options);
        }
      },
    );
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("shows the alert while the server does not push to this device", () => {
    vi.mocked(pushActive).mockReturnValue(false);

    sendNotification({ title: "Agent finished", body: "#1 Add login" });

    expect(shown).toHaveBeenCalledWith("Agent finished", { body: "#1 Add login" });
  });

  it("leaves the alert to the server's push while push is on", () => {
    vi.mocked(pushActive).mockReturnValue(true);

    sendNotification({ title: "Agent finished", body: "#1 Add login" });

    expect(shown).not.toHaveBeenCalled();
  });
});
