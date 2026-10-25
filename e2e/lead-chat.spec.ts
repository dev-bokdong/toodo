import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "./fixtures";

test.use({ serverOptions: { lead: true } });

test("the lead's messages come from the lead and each message shows how far it got, live", async ({ app, server }) => {
  const button = app.getByRole("button", { name: /^Lead chat/ });
  await button.click();
  const panel = app.getByRole("dialog", { name: "Lead chat" });
  const chat = panel.getByRole("region", { name: "Chat with the lead" });
  // No name or picture configured: "Lead" with a generic icon.
  await expect(chat.getByRole("heading", { name: "Lead" })).toBeVisible();
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
  await expect(button).toHaveAccessibleName(/Lead is typing/, { timeout: 15_000 });

  // Opened again: the message was seen, and the lead's typing bubble shows until the reply is stored.
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
  const reply = log.getByRole("listitem", { name: "Lead" });
  await expect(reply).toContainText("On it.");
  await expect(reply.locator("[data-lead-avatar] svg")).toBeVisible();
  await expect(reply.locator("img")).toHaveCount(0);
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
  await expect(panel.getByRole("listitem", { name: "Lead" })).toContainText("Anything else?");
  await expect(button).toHaveAccessibleName("Lead chat");
  await expect(input).toHaveValue("Half a thought");

  // On a phone the chat is a full-screen sheet with its own close button.
  await app.setViewportSize({ width: 390, height: 844 });
  await expect.poll(async () => panel.boundingBox()).toEqual({ x: 0, y: 0, width: 390, height: 844 });
  await panel.getByRole("button", { name: "Close lead chat" }).click();
  await expect(panel).toBeHidden();
  await expect(button).toBeFocused();
});

/** A 1x1 PNG for `TOODO_LEAD_AVATAR`, written before the server of the tests below starts. */
const avatarDir = mkdtempSync(path.join(tmpdir(), "toodo-e2e-avatar-"));
const avatar = path.join(avatarDir, "lead.png");
writeFileSync(
  avatar,
  Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==", "base64"),
);
test.afterAll(() => rmSync(avatarDir, { recursive: true, force: true }));

test.describe("with an outside lead", () => {
  test.use({
    serverOptions: { lead: "external", env: { TOODO_LEAD_NAME: "Planner", TOODO_LEAD_AVATAR: avatar } },
  });

  test("shows the configured lead, keeps messages while it is away, and its typing", async ({ app, server }) => {
    const button = app.getByRole("button", { name: /^Lead chat/ });
    await button.click();
    const chat = app.getByRole("dialog", { name: "Lead chat" }).getByRole("region", { name: "Chat with the lead" });
    await expect(chat.getByRole("heading", { name: "Planner" })).toBeVisible();
    const picture = chat.locator("header img[data-lead-avatar]");
    await expect(picture).toHaveAttribute("src", /^data:image\/png;base64,/);
    await expect(picture).toHaveJSProperty("naturalWidth", 1);

    // Out of touch: the header and the composer say so, and a message waits in the inbox.
    const status = chat.getByRole("status", { name: "Lead status" });
    await expect(status).toHaveText("Disconnected");
    const note = chat.getByText("Planner is disconnected. Messages wait here until it connects.");
    await expect(note).toBeVisible();
    const input = chat.getByRole("textbox", { name: "Message to the lead" });
    await expect(input).toHaveAttribute("placeholder", "Planner is disconnected; your message waits for it");
    await input.fill("Plan the release");
    await input.press("Enter");
    const log = chat.getByRole("log", { name: "Messages" });
    const mine = log.getByRole("listitem", { name: "You" }).filter({ hasText: "Plan the release" });
    await expect(mine.locator("[data-receipt]")).toHaveText("Waiting");

    // The outside lead gets in touch, then takes the message.
    const peeked = await server.mcp<{ items: { id: string }[] }>("peek_pending", {}, "lead");
    expect(peeked.items).toHaveLength(1);
    await expect(status).toHaveText("Connected");
    await expect(note).toBeHidden();
    await expect(input).toHaveAttribute("placeholder", "Message Planner (Enter sends, Shift+Enter for a new line)");
    await expect(mine.locator("[data-receipt]")).toHaveText("Waiting");
    // Taking it marks it delivered and seen at once.
    await server.mcp("take_pending", { ids: [peeked.items[0].id] }, "lead");
    await expect(mine.locator("[data-receipt]")).toHaveText("Seen");

    // Its typing reports drive the dots, on the closed button and in the open chat.
    const typing = log.getByRole("listitem", { name: "Reply is being written" });
    await server.mcp("set_typing", { typing: true }, "lead");
    await expect(typing).toBeVisible();
    await expect(typing.locator("img[data-lead-avatar]")).toBeVisible();
    await input.press("Escape");
    await expect(button).toHaveAccessibleName("Lead chat, Planner is typing");
    await server.mcp("set_typing", { typing: false }, "lead");
    await expect(button).toHaveAccessibleName("Lead chat");
    await button.click();
    await expect(typing).toHaveCount(0);
  });
});
