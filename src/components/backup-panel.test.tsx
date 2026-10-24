import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { BackupPanel } from "@/components/backup-panel";
import { commands, type BackupEntry } from "@/lib/bindings";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));

vi.mock("@/lib/bindings", () => ({
  commands: {
    listBackups: vi.fn(),
    getBackupSettings: vi.fn(),
    saveBackupSettings: vi.fn(),
    exportDatabase: vi.fn(),
    restoreDatabase: vi.fn(),
    restoreBackup: vi.fn(),
  },
}));

const mocked = vi.mocked(commands);
const mockedOpen = vi.mocked(openDialog);
const mockedSave = vi.mocked(saveDialog);

const automatic: BackupEntry = {
  path: "C:\\Users\\me\\AppData\\Roaming\\dev.toodo.app\\backups\\toodo-auto-20261014-093005.sqlite3",
  kind: "automatic",
  createdAt: 1_791_970_205,
  sizeBytes: 40_960,
};

const safety: BackupEntry = {
  path: "C:\\Users\\me\\AppData\\Roaming\\dev.toodo.app\\backups\\toodo-pre-restore-20261014-100000.sqlite3",
  kind: "preRestore",
  createdAt: 1_791_972_000,
  sizeBytes: 40_960,
};

function renderPanel() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const invalidate = vi.spyOn(queryClient, "invalidateQueries");
  render(
    <QueryClientProvider client={queryClient}>
      <BackupPanel />
    </QueryClientProvider>,
  );
  return { invalidate };
}

describe("BackupPanel", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    mocked.listBackups.mockResolvedValue({ status: "ok", data: [automatic] });
    mocked.getBackupSettings.mockResolvedValue({ enabled: true, intervalHours: 24, keep: 7 });
  });

  it("exports the database to the file picked in the save dialog", async () => {
    mockedSave.mockResolvedValue("D:\\backups\\toodo.sqlite3");
    mocked.exportDatabase.mockResolvedValue({ status: "ok", data: null });
    renderPanel();

    fireEvent.click(screen.getByRole("button", { name: "Export..." }));

    expect(await screen.findByRole("status")).toHaveTextContent("Exported to D:\\backups\\toodo.sqlite3");
    expect(mocked.exportDatabase).toHaveBeenCalledWith("D:\\backups\\toodo.sqlite3");
  });

  it("does nothing when the save dialog is cancelled", async () => {
    mockedSave.mockResolvedValue(null);
    renderPanel();

    fireEvent.click(screen.getByRole("button", { name: "Export..." }));

    await waitFor(() => expect(mockedSave).toHaveBeenCalled());
    expect(mocked.exportDatabase).not.toHaveBeenCalled();
  });

  it("restores a picked file only after confirmation and refetches everything", async () => {
    mockedOpen.mockResolvedValue("D:\\backups\\toodo.sqlite3");
    mocked.restoreDatabase.mockResolvedValue({ status: "ok", data: safety });
    const { invalidate } = renderPanel();

    fireEvent.click(screen.getByRole("button", { name: "Restore..." }));
    const dialog = await screen.findByRole("alertdialog", { name: "Confirm restore" });
    expect(dialog).toHaveTextContent("toodo.sqlite3");
    expect(mocked.restoreDatabase).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Replace data" }));

    expect(await screen.findByRole("status")).toHaveTextContent(
      "The previous data was saved as toodo-pre-restore-20261014-100000.sqlite3",
    );
    expect(mocked.restoreDatabase).toHaveBeenCalledWith("D:\\backups\\toodo.sqlite3");
    expect(invalidate).toHaveBeenCalledWith();
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
  });

  it("restores a listed backup by name and shows a rejected backup's error", async () => {
    mocked.restoreBackup.mockResolvedValue({
      status: "error",
      error: { kind: "invalidInput", message: "invalid backup x: not a Toodo database" },
    });
    renderPanel();

    const list = await screen.findByRole("list", { name: "Saved backups" });
    fireEvent.click(await within(list).findByRole("button", { name: /^Restore this backup from/ }));
    fireEvent.click(screen.getByRole("button", { name: "Replace data" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("not a Toodo database");
    expect(mocked.restoreBackup).toHaveBeenCalledWith("toodo-auto-20261014-093005.sqlite3");
    expect(mocked.restoreDatabase).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
  });

  it("saves the automatic backup schedule", async () => {
    mocked.saveBackupSettings.mockResolvedValue({
      status: "ok",
      data: { enabled: true, intervalHours: 6, keep: 3 },
    });
    renderPanel();

    fireEvent.change(await screen.findByLabelText("Hours between backups"), { target: { value: "6" } });
    fireEvent.change(screen.getByLabelText("Backups to keep"), { target: { value: "3" } });
    fireEvent.click(screen.getByRole("button", { name: "Save schedule" }));

    await waitFor(() =>
      expect(mocked.saveBackupSettings).toHaveBeenCalledWith({ enabled: true, intervalHours: 6, keep: 3 }),
    );
    await waitFor(() => expect(mocked.listBackups).toHaveBeenCalledTimes(2));
  });
});
