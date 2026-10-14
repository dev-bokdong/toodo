import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type DownloadEvent, type Update } from "@tauri-apps/plugin-updater";
import { UpdateBanner } from "@/components/update-banner";

vi.mock("@tauri-apps/plugin-updater", () => ({ check: vi.fn() }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn() }));

const mockedCheck = vi.mocked(check);
const mockedRelaunch = vi.mocked(relaunch);

function availableUpdate(downloadAndInstall: Update["downloadAndInstall"]): Update {
  return { version: "0.2.0", currentVersion: "0.1.0", downloadAndInstall } as Update;
}

function renderBanner() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={queryClient}>
      <UpdateBanner />
    </QueryClientProvider>,
  );
}

describe("UpdateBanner", () => {
  beforeEach(() => {
    vi.resetAllMocks();
  });

  it("stays hidden when no update is available", async () => {
    mockedCheck.mockResolvedValue(null);
    renderBanner();

    await waitFor(() => expect(mockedCheck).toHaveBeenCalled());
    expect(screen.queryByRole("status", { name: "App update" })).not.toBeInTheDocument();
  });

  it("stays hidden when the update check fails", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    mockedCheck.mockRejectedValue(new Error("offline"));
    renderBanner();

    await waitFor(() => expect(warn).toHaveBeenCalledWith("Update check failed:", expect.any(Error)));
    expect(screen.queryByRole("status", { name: "App update" })).not.toBeInTheDocument();
    warn.mockRestore();
  });

  it("installs the update and relaunches the app", async () => {
    const downloadAndInstall = vi.fn(async (onEvent?: (event: DownloadEvent) => void) => {
      onEvent?.({ event: "Started", data: { contentLength: 100 } });
      onEvent?.({ event: "Progress", data: { chunkLength: 100 } });
      onEvent?.({ event: "Finished" });
    });
    mockedCheck.mockResolvedValue(availableUpdate(downloadAndInstall));
    mockedRelaunch.mockResolvedValue();
    renderBanner();

    const banner = await screen.findByRole("status", { name: "App update" });
    expect(banner).toHaveTextContent("Toodo 0.2.0 is available");
    expect(banner).toHaveTextContent("You are running 0.1.0.");
    fireEvent.click(screen.getByRole("button", { name: "Install and restart" }));

    await waitFor(() => expect(mockedRelaunch).toHaveBeenCalledTimes(1));
    expect(downloadAndInstall).toHaveBeenCalledTimes(1);
  });

  it("shows the error and does not relaunch when installing fails", async () => {
    mockedCheck.mockResolvedValue(availableUpdate(vi.fn().mockRejectedValue(new Error("signature mismatch"))));
    renderBanner();

    fireEvent.click(await screen.findByRole("button", { name: "Install and restart" }));

    expect(await screen.findByText("Update failed: signature mismatch")).toBeInTheDocument();
    expect(mockedRelaunch).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Install and restart" })).toBeEnabled();
  });
});
