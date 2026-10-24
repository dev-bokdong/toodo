import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Button } from "@/components/ui/button";
import { listDevices, revokeDevice, type Device } from "@/web/devices";

const devicesKey = ["devices"];

function formatTime(seconds: number): string {
  return new Date(seconds * 1000).toLocaleString();
}

function DeviceRow({ device }: { device: Device }) {
  const queryClient = useQueryClient();
  const [confirming, setConfirming] = useState(false);
  const revoke = useMutation({
    mutationFn: () => revokeDevice(device.id),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: devicesKey }),
  });
  const revoked = device.revokedAt !== null;

  return (
    <li aria-label={`Device ${device.label}`} data-revoked={revoked} className="flex flex-col gap-0.5">
      <span className="flex items-center gap-1.5">
        <span className={revoked ? "truncate text-muted-foreground line-through" : "truncate font-medium"}>
          {device.label}
        </span>
        {device.current && <span className="text-muted-foreground">(this device)</span>}
      </span>
      <span className="text-muted-foreground">
        {device.revokedAt !== null
          ? `Revoked ${formatTime(device.revokedAt)}`
          : `Last seen ${formatTime(device.lastSeenAt)}`}
      </span>
      {!revoked &&
        (confirming ? (
          <div role="alertdialog" aria-label={`Confirm revoke of ${device.label}`} className="flex flex-col gap-1 rounded-md border border-destructive/40 p-2">
            <p>
              {device.current
                ? "This browser signs out and needs the shared token to sign in again."
                : "The device's token stops working for good."}
            </p>
            <div className="flex gap-1">
              <Button size="xs" variant="destructive" disabled={revoke.isPending} onClick={() => revoke.mutate()}>
                Revoke
              </Button>
              <Button size="xs" variant="ghost" onClick={() => setConfirming(false)}>
                Keep
              </Button>
            </div>
          </div>
        ) : (
          <Button size="xs" variant="outline" className="self-start" aria-label={`Revoke ${device.label}`} onClick={() => setConfirming(true)}>
            Revoke
          </Button>
        ))}
      {revoke.error && (
        <p role="alert" className="break-words text-destructive">
          {revoke.error.message}
        </p>
      )}
    </li>
  );
}

export function DevicesPanel() {
  const devices = useQuery({ queryKey: devicesKey, queryFn: listDevices });
  const active = (devices.data ?? []).filter((device) => device.revokedAt === null).length;

  return (
    <details className="group rounded-lg border p-2">
      <summary className="cursor-pointer text-xs font-medium uppercase text-muted-foreground">
        Devices
        {devices.data && <span className="ml-1.5 normal-case">{active} signed in</span>}
      </summary>
      <section aria-label="Devices" className="mt-2 flex flex-col gap-2 text-xs">
        <p className="text-muted-foreground">
          Each browser signs in with the shared token once and then keeps its own token. Revoke a lost device here.
        </p>
        {devices.error && (
          <p role="alert" className="break-words text-destructive">
            {devices.error.message}
          </p>
        )}
        {devices.data && (
          <ul aria-label="Signed-in devices" className="flex flex-col gap-2">
            {devices.data.map((device) => (
              <DeviceRow key={device.id} device={device} />
            ))}
          </ul>
        )}
      </section>
    </details>
  );
}
