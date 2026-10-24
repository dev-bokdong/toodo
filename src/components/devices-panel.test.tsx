import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { DevicesPanel } from "@/components/devices-panel";
import { listDevices, revokeDevice, type Device } from "@/web/devices";

vi.mock("@/web/devices", () => ({ listDevices: vi.fn(), revokeDevice: vi.fn() }));

const phone: Device = { id: 1, label: "Phone", createdAt: 1_791_970_000, lastSeenAt: 1_791_970_205, revokedAt: null, current: false };
const laptop: Device = { id: 2, label: "Laptop", createdAt: 1_791_960_000, lastSeenAt: 1_791_971_000, revokedAt: null, current: true };
const tablet: Device = { id: 3, label: "Tablet", createdAt: 1_791_900_000, lastSeenAt: 1_791_900_000, revokedAt: 1_791_950_000, current: false };

function renderPanel() {
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <DevicesPanel />
    </QueryClientProvider>,
  );
}

describe("DevicesPanel", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(listDevices).mockResolvedValue([laptop, phone, tablet]);
  });

  it("lists devices with this one marked and revoked ones without a revoke button", async () => {
    renderPanel();

    const list = await screen.findByRole("list", { name: "Signed-in devices" });
    expect(within(list).getByRole("listitem", { name: "Device Laptop" })).toHaveTextContent("(this device)");
    expect(within(list).getByRole("listitem", { name: "Device Phone" })).toHaveTextContent("Last seen");
    const revoked = within(list).getByRole("listitem", { name: "Device Tablet" });
    expect(revoked).toHaveTextContent("Revoked");
    expect(within(revoked).queryByRole("button")).toBeNull();
    expect(screen.getByText("2 signed in")).toBeInTheDocument();
  });

  it("revokes a device only after confirmation and refetches the list", async () => {
    vi.mocked(revokeDevice).mockResolvedValue({ ...phone, revokedAt: 1_791_972_000 });
    renderPanel();

    fireEvent.click(await screen.findByRole("button", { name: "Revoke Phone" }));
    const dialog = screen.getByRole("alertdialog", { name: "Confirm revoke of Phone" });
    expect(revokeDevice).not.toHaveBeenCalled();
    vi.mocked(listDevices).mockResolvedValue([laptop, { ...phone, revokedAt: 1_791_972_000 }, tablet]);
    fireEvent.click(within(dialog).getByRole("button", { name: "Revoke" }));

    await waitFor(() => expect(screen.getByRole("listitem", { name: "Device Phone" })).toHaveTextContent("Revoked"));
    expect(screen.getByText("1 signed in")).toBeInTheDocument();
    expect(vi.mocked(revokeDevice).mock.calls[0][0]).toBe(1);
  });

  it("warns that revoking this device signs this browser out", async () => {
    renderPanel();

    fireEvent.click(await screen.findByRole("button", { name: "Revoke Laptop" }));

    expect(screen.getByRole("alertdialog", { name: "Confirm revoke of Laptop" })).toHaveTextContent(
      "This browser signs out",
    );
  });
});
