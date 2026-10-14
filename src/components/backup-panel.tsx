import { useState, type FormEvent } from "react";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { Button } from "@/components/ui/button";
import type { BackupEntry, BackupSettings } from "@/lib/bindings";
import {
  useBackupSettings,
  useBackups,
  useExportDatabase,
  useRestoreDatabase,
  useSaveBackupSettings,
} from "@/lib/queries";

const inputClass = "h-7 w-16 rounded-md border bg-background px-2 text-sm";

const databaseFilters = [{ name: "Toodo database", extensions: ["sqlite3", "db"] }];

function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

function formatTime(seconds: number): string {
  return new Date(seconds * 1000).toLocaleString();
}

function formatSize(bytes: number): string {
  return bytes < 1024 * 1024 ? `${Math.ceil(bytes / 1024)} KB` : `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

const kindLabels: Record<BackupEntry["kind"], string> = {
  automatic: "Automatic",
  preRestore: "Before restore",
};

export function BackupPanel() {
  const backups = useBackups();
  const exportDatabase = useExportDatabase();
  const restoreDatabase = useRestoreDatabase();
  const [confirming, setConfirming] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [dialogError, setDialogError] = useState<string | null>(null);

  async function exportToFile() {
    setNotice(null);
    setDialogError(null);
    try {
      const date = new Date().toISOString().slice(0, 10);
      const path = await saveDialog({
        title: "Export database",
        defaultPath: `toodo-backup-${date}.sqlite3`,
        filters: databaseFilters,
      });
      if (typeof path !== "string") return;
      exportDatabase.mutate(path, { onSuccess: () => setNotice(`Exported to ${path}`) });
    } catch (error) {
      setDialogError(`Could not open the save dialog: ${String(error)}`);
    }
  }

  async function pickBackup() {
    setNotice(null);
    setDialogError(null);
    try {
      const path = await openDialog({
        title: "Restore database",
        multiple: false,
        directory: false,
        filters: databaseFilters,
      });
      if (typeof path === "string") setConfirming(path);
    } catch (error) {
      setDialogError(`Could not open the file dialog: ${String(error)}`);
    }
  }

  function restore(path: string) {
    setNotice(null);
    restoreDatabase.mutate(path, {
      onSuccess: (safety) => {
        setConfirming(null);
        setNotice(`Restored ${fileName(path)}. The previous data was saved as ${fileName(safety.path)}.`);
      },
    });
  }

  const error = exportDatabase.error ?? restoreDatabase.error ?? backups.error;

  return (
    <details className="group rounded-lg border p-2">
      <summary className="cursor-pointer text-xs font-medium uppercase text-muted-foreground">Backups</summary>
      <section aria-label="Backups" className="mt-2 flex flex-col gap-2">
        <div className="flex gap-1">
          <Button size="sm" variant="outline" onClick={exportToFile} disabled={exportDatabase.isPending}>
            {exportDatabase.isPending ? "Exporting..." : "Export..."}
          </Button>
          <Button size="sm" variant="outline" onClick={pickBackup} disabled={restoreDatabase.isPending}>
            Restore...
          </Button>
        </div>
        {confirming && (
          <div role="alertdialog" aria-label="Confirm restore" className="flex flex-col gap-1 rounded-md border border-destructive/40 p-2">
            <p className="text-xs break-words">
              Replace all workspaces and tasks with <span className="font-medium">{fileName(confirming)}</span>? The
              current data is backed up first.
            </p>
            <div className="flex gap-1">
              <Button size="xs" variant="destructive" onClick={() => restore(confirming)} disabled={restoreDatabase.isPending}>
                {restoreDatabase.isPending ? "Restoring..." : "Replace data"}
              </Button>
              <Button size="xs" variant="ghost" onClick={() => setConfirming(null)} disabled={restoreDatabase.isPending}>
                Cancel
              </Button>
            </div>
          </div>
        )}
        {notice && (
          <p role="status" className="text-xs break-words text-muted-foreground">
            {notice}
          </p>
        )}
        {(dialogError ?? error) && (
          <p role="alert" className="text-xs break-words text-destructive">
            {dialogError ?? error?.message}
          </p>
        )}
        <AutoBackupSettings />
        <ul aria-label="Saved backups" className="flex max-h-48 flex-col gap-1 overflow-y-auto">
          {backups.data?.length === 0 && <li className="text-xs text-muted-foreground">No backups yet.</li>}
          {backups.data?.map((backup) => (
            <li key={backup.path} className="flex items-center justify-between gap-1 text-xs" title={backup.path}>
              <span className="min-w-0">
                <span className="block truncate">{formatTime(backup.createdAt)}</span>
                <span className="text-muted-foreground">
                  {kindLabels[backup.kind]} · {formatSize(backup.sizeBytes)}
                </span>
              </span>
              <Button
                size="xs"
                variant="ghost"
                aria-label={`Restore backup from ${formatTime(backup.createdAt)}`}
                onClick={() => {
                  setNotice(null);
                  setConfirming(backup.path);
                }}
              >
                Restore
              </Button>
            </li>
          ))}
        </ul>
      </section>
    </details>
  );
}

function AutoBackupSettings() {
  const settings = useBackupSettings();
  if (!settings.data) return null;
  const { enabled, intervalHours, keep } = settings.data;
  return <AutoBackupForm key={`${enabled}-${intervalHours}-${keep}`} initial={settings.data} />;
}

function AutoBackupForm({ initial }: { initial: BackupSettings }) {
  const save = useSaveBackupSettings();
  const [enabled, setEnabled] = useState(initial.enabled);
  const [intervalHours, setIntervalHours] = useState(String(initial.intervalHours));
  const [keep, setKeep] = useState(String(initial.keep));

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    save.mutate({ enabled, intervalHours: Number(intervalHours), keep: Number(keep) });
  }

  return (
    <form onSubmit={submit} aria-label="Automatic backups" className="flex flex-col gap-1 text-xs">
      <label className="flex items-center gap-2">
        <input type="checkbox" checked={enabled} onChange={(e) => setEnabled(e.target.checked)} />
        Back up automatically
      </label>
      <label className="flex items-center gap-2">
        Every
        <input
          className={inputClass}
          type="number"
          min={1}
          max={720}
          aria-label="Hours between backups"
          value={intervalHours}
          onChange={(e) => setIntervalHours(e.target.value)}
          disabled={!enabled}
          required
        />
        hours
      </label>
      <label className="flex items-center gap-2">
        Keep
        <input
          className={inputClass}
          type="number"
          min={1}
          max={100}
          aria-label="Backups to keep"
          value={keep}
          onChange={(e) => setKeep(e.target.value)}
          disabled={!enabled}
          required
        />
        latest
      </label>
      {save.isError && (
        <p role="alert" className="text-destructive">
          {save.error.message}
        </p>
      )}
      <Button type="submit" size="xs" variant="secondary" className="self-start" disabled={save.isPending}>
        {save.isPending ? "Saving..." : "Save schedule"}
      </Button>
    </form>
  );
}
