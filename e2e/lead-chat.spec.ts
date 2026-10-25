import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { expect, test } from "./fixtures";

test.use({ serverOptions: { lead: true } });

test("the lead's messages come from Dori and each message shows how far it got, live", async ({ app, server }) => {
  const button = app.getByRole("button", { name: /^Lead chat/ });
  await button.click();
  const panel = app.getByRole("dialog", { name: "Lead chat" });
  const chat = panel.getByRole("region", { name: "Chat with the lead" });
  await expect(chat.getByRole("heading", { name: "Dori" })).toBeVisible();
  const input = chat.getByRole("textbox", { name: "Message to the lead" });
  await expect(input).toBeFocused();
  // The supervisor checks the fake lead every few seconds.
  await expect(chat.getByRole("status", { name: "Lead status" })).toHaveText("Ready", { timeout: 15_000 });

  const log = chat.getByRole("log", { name: "Messages" });
  const mine = (text: string) => log.getByRole("listitem", { name: "You" }).filter({ hasText: text });

  // Its ID typed into the lead's pane, never its text: delivered.
  await input.fill("Plan the release");
  await input.press("Enter");
  await expect(mine("Plan the release").locator("[data-receipt]")).toHaveText("Delivered");
  const typed = readFileSync(path.join(server.herdrDir, "typed.log"), "utf8");
  expect(typed).toMatch(/^\[Toodo inbox\] chat-\d+ - take them with the Toodo MCP tool take_pending$/m);
  expect(typed).not.toContain("Plan the release");

  // Closed, the button shows the lead composing its reply.
  await input.press("Escape");
  await expect(panel).toBeHidden();
  await expect(button).toBeFocused();
  writeFileSync(path.join(server.herdrDir, "agent-status"), "working");
  await expect(button).toHaveAccessibleName(/Dori is typing/, { timeout: 15_000 });

  // Opened again: the message was seen, and Dori's typing bubble shows until the reply is stored.
  await button.click();
  await expect(mine("Plan the release").locator("[data-receipt]")).toHaveText("Seen");
  const typing = log.getByRole("listitem", { name: "Reply is being written" });
  await expect(typing).toBeVisible();

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

test("the chat floats over every view, keeps its draft, and counts replies while closed", async ({ app, server }) => {
  // Nothing in the menu leads to a chat page any more.
  await expect(app.getByRole("navigation").getByRole("button", { name: /chat/i })).toHaveCount(0);

  const button = app.getByRole("button", { name: /^Lead chat/ });
  await button.click();
  const panel = app.getByRole("dialog", { name: "Lead chat" });
  await expect(panel).toBeVisible();
  const box = await panel.boundingBox();
  expect(box && [box.width, box.height]).toEqual([380, 560]);

  // Moving between views leaves the panel open with the unsent message in it.
  const input = panel.getByRole("textbox", { name: "Message to the lead" });
  await input.fill("Half a thought");
  await app.getByRole("button", { name: "Review", exact: true }).click();
  await expect(app.getByRole("heading", { name: "Review" })).toBeVisible();
  await expect(panel).toBeVisible();
  await app.getByRole("button", { name: "Shared memory", exact: true }).click();
  await expect(input).toHaveValue("Half a thought");

  // Closed with the button: a reply shows as unread on it, and opening the chat reads it.
  await button.click();
  await expect(panel).toBeHidden();
  await server.mcp("reply_in_chat", { text: "Anything else?" });
  await expect(button).toHaveAccessibleName("Lead chat, 1 unread");
  await button.click();
  await expect(panel.getByRole("listitem", { name: "Dori" })).toContainText("Anything else?");
  await expect(button).toHaveAccessibleName("Lead chat");
  await expect(input).toHaveValue("Half a thought");

  // On a phone the chat is a full-screen sheet with its own close button.
  await app.setViewportSize({ width: 390, height: 844 });
  await expect.poll(async () => panel.boundingBox()).toEqual({ x: 0, y: 0, width: 390, height: 844 });
  await panel.getByRole("button", { name: "Close lead chat" }).click();
  await expect(panel).toBeHidden();
  await expect(button).toBeFocused();
});
