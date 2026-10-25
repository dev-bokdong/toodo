import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { expect, test } from "./fixtures";

test.use({ serverOptions: { lead: true } });

test("the lead's messages come from Dori and each message shows how far it got, live", async ({ app, server }) => {
  await app.getByRole("button", { name: "Lead chat" }).click();
  const chat = app.getByRole("region", { name: "Chat with the lead" });
  await expect(chat.getByRole("heading", { name: "Dori" })).toBeVisible();
  // The supervisor checks the fake lead every few seconds.
  await expect(chat.getByRole("status", { name: "Lead status" })).toHaveText("Ready", { timeout: 15_000 });

  const log = chat.getByRole("log", { name: "Messages" });
  const mine = (text: string) => log.getByRole("listitem", { name: "You" }).filter({ hasText: text });
  const input = chat.getByRole("textbox", { name: "Message to the lead" });

  // Typed into the lead's pane: delivered.
  await input.fill("Plan the release");
  await input.press("Enter");
  await expect(mine("Plan the release").locator("[data-receipt]")).toHaveText("Delivered");
  expect(readFileSync(path.join(server.herdrDir, "typed.log"), "utf8")).toContain("[Toodo chat] Plan the release");

  // The agent starts a turn: the message is seen, without a reload.
  writeFileSync(path.join(server.herdrDir, "agent-status"), "working");
  await expect(mine("Plan the release").locator("[data-receipt]")).toHaveText("Seen", { timeout: 15_000 });
  // The lead is composing its reply: Dori's typing bubble shows until the reply is stored.
  const typing = log.getByRole("listitem", { name: "Reply is being written" });
  await expect(typing).toBeVisible({ timeout: 15_000 });

  // Typed in while the agent is still working: delivered until the lead replies.
  await input.fill("Then tag it");
  await input.press("Enter");
  await expect(mine("Then tag it").locator("[data-receipt]")).toHaveText("Delivered");
  await server.mcp("reply_in_chat", { text: "On it." });
  await expect(typing).toHaveCount(0);
  const reply = log.getByRole("listitem", { name: "Dori" });
  await expect(reply).toContainText("On it.");
  await expect(reply.locator("img")).toBeVisible();
  await expect(reply.locator("img")).toHaveJSProperty("naturalWidth", 80);
  await expect(mine("Then tag it").locator("[data-receipt]")).toHaveText("Seen");
});
