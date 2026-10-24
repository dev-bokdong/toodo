import { lazy, Suspense, useEffect, useState, type FormEvent } from "react";
import { BookMarked, Pencil } from "lucide-react";

import { CollapsibleForm } from "@/components/collapsible-form";
import { MarkdownEditor } from "@/components/markdown-editor";
import { EmptyState } from "@/components/task-list";
import { Button } from "@/components/ui/button";
import { DiscardConfirm, useDiscardGuard } from "@/components/discard-guard";
import { inputClass } from "@/components/workspace-list";
import type { MemoryContent, MemoryEntry, MemoryKind, Workspace } from "@/lib/bindings";
import { useAddMemory, useMemoryEntries, useSupersedeMemory, useUpdateMemory } from "@/lib/queries";
import { formatDate } from "@/lib/timeline";
import { cn } from "@/lib/utils";

const Markdown = lazy(() => import("@/components/markdown"));

const kinds: { kind: MemoryKind; label: string }[] = [
  { kind: "decision", label: "Decision" },
  { kind: "rule", label: "Rule" },
  { kind: "note", label: "Note" },
  { kind: "summary", label: "Summary" },
];

const kindLabel = (kind: MemoryKind) => kinds.find((entry) => entry.kind === kind)?.label ?? kind;

export type MemoryDraft = Pick<MemoryContent, "kind" | "title" | "body">;

const emptyDraft: MemoryDraft = { kind: "decision", title: "", body: "" };

/**
 * The project memory of a workspace, or the memory shared by every workspace when
 * `workspace` is null: decisions, rules, notes, and summaries agents read before they act.
 */
export function MemoryPanel({
  workspace,
  draft = emptyDraft,
  onDraftChange,
}: {
  workspace: Workspace | null;
  /** The new entry being written, owned by the caller so it survives switching away. */
  draft?: MemoryDraft;
  onDraftChange?: (draft: MemoryDraft) => void;
}) {
  const [showSuperseded, setShowSuperseded] = useState(false);
  const entries = useMemoryEntries(workspace?.id ?? null, showSuperseded);
  const add = useAddMemory();
  const [formKey, setFormKey] = useState(0);
  const title = workspace ? `Memory of ${workspace.name}` : "Shared memory";

  return (
    <section aria-label={title} className="flex flex-1 flex-col gap-4">
      <header>
        <h2 className="text-xl font-semibold tracking-tight">{title}</h2>
        <p className="text-sm text-muted-foreground">
          {workspace
            ? "What agents should know about this workspace before they act: decisions, rules, notes, and summaries."
            : "Knowledge every workspace shares: standing preferences and rules across projects."}
        </p>
      </header>

      <EntryForm
        key={formKey}
        label="New memory entry"
        submitLabel="Add entry"
        collapsible
        initial={{ ...draft, taskId: null }}
        onChange={onDraftChange}
        pending={add.isPending}
        error={add.error}
        onSubmit={(content) =>
          add.mutate(
            { workspace: workspace?.id ?? null, content },
            {
              onSuccess: () => {
                onDraftChange?.(emptyDraft);
                setFormKey((key) => key + 1);
              },
            },
          )
        }
      />

      <label className="flex items-center gap-2 text-sm">
        <input type="checkbox" checked={showSuperseded} onChange={(e) => setShowSuperseded(e.target.checked)} />
        Show superseded entries
      </label>

      {entries.error ? (
        <p role="alert" className="text-sm text-destructive">
          {entries.error.message}
        </p>
      ) : entries.isPending ? (
        <p role="status" className="text-sm text-muted-foreground">
          Loading memory...
        </p>
      ) : entries.data?.length === 0 ? (
        <EmptyState
          icon={<BookMarked />}
          title="No memory entries yet."
          hint="Agents add entries through MCP as they learn; you can add them here too."
        />
      ) : (
        entries.data && (
          <ul aria-label="Memory entries" className="flex flex-col gap-2">
            {entries.data.map((entry) => (
              <MemoryItem key={entry.id} entry={entry} />
            ))}
          </ul>
        )
      )}
    </section>
  );
}

function MemoryItem({ entry }: { entry: MemoryEntry }) {
  const [mode, setMode] = useState<"view" | "edit" | "supersede">("view");
  const update = useUpdateMemory();
  const supersede = useSupersedeMemory();
  const superseded = entry.supersededBy !== null;

  return (
    <li
      aria-label={`Memory entry #${entry.id}`}
      data-superseded={superseded || undefined}
      className={cn("flex flex-col gap-2 rounded-lg border p-3", superseded && "opacity-60")}
    >
      <div className="flex flex-wrap items-center gap-2">
        <span className="rounded-full border px-2 py-0.5 text-xs font-medium" data-memory-kind={entry.kind}>
          {kindLabel(entry.kind)}
        </span>
        <h3 className="min-w-0 flex-1 font-medium break-words">{entry.title}</h3>
        {!superseded && mode === "view" && (
          <div className="flex gap-1">
            <Button
              size="icon-xs"
              variant="ghost"
              aria-label={`Edit memory entry #${entry.id}`}
              title="Edit"
              onClick={() => setMode("edit")}
            >
              <Pencil />
            </Button>
            <Button
              size="xs"
              variant="ghost"
              aria-label={`Supersede memory entry #${entry.id}`}
              onClick={() => setMode("supersede")}
            >
              Supersede
            </Button>
          </div>
        )}
      </div>
      {mode === "view" ? (
        entry.body && (
          <Suspense fallback={<p className="text-sm whitespace-pre-line">{entry.body}</p>}>
            <Markdown source={entry.body} />
          </Suspense>
        )
      ) : (
        <EntryForm
          label={mode === "edit" ? `Edit of #${entry.id}` : `Replacement of #${entry.id}`}
          submitLabel={mode === "edit" ? "Save" : "Supersede"}
          initial={mode === "edit" ? entry : { ...entry, body: "" }}
          pending={update.isPending || supersede.isPending}
          error={mode === "edit" ? update.error : supersede.error}
          onCancel={() => setMode("view")}
          onSubmit={(content) => {
            const done = { onSuccess: () => setMode("view") };
            if (mode === "edit") update.mutate({ id: entry.id, content }, done);
            else supersede.mutate({ id: entry.id, content }, done);
          }}
        />
      )}
      <p className="text-xs text-muted-foreground">
        #{entry.id} · updated {formatDate(entry.updatedAt)}
        {entry.taskId !== null && ` · from task #${entry.taskId}`}
        {superseded && ` · superseded by #${entry.supersededBy}`}
      </p>
    </li>
  );
}

function EntryForm({
  label,
  submitLabel,
  collapsible = false,
  initial,
  pending,
  error,
  onSubmit,
  onChange,
  onCancel,
}: {
  label: string;
  submitLabel: string;
  /** Rest as one line with the submit button until the form is used. */
  collapsible?: boolean;
  initial?: Pick<MemoryEntry, "kind" | "title" | "body" | "taskId">;
  pending: boolean;
  error: Error | null;
  onSubmit: (content: MemoryContent) => void;
  onChange?: (draft: MemoryDraft) => void;
  onCancel?: () => void;
}) {
  const [kind, setKind] = useState<MemoryKind>(initial?.kind ?? "decision");
  const [title, setTitle] = useState(initial?.title ?? "");
  const [body, setBody] = useState(initial?.body ?? "");
  const dirty =
    kind !== (initial?.kind ?? "decision") || title !== (initial?.title ?? "") || body !== (initial?.body ?? "");
  const guard = useDiscardGuard(dirty, () => onCancel?.(), pending);
  const [expanded, setExpanded] = useState(!collapsible);
  useEffect(() => onChange?.({ kind, title, body }), [kind, title, body]);

  const submit = (event: FormEvent) => {
    event.preventDefault();
    onSubmit({ kind, title, body, taskId: initial?.taskId ?? null });
  };

  const submitButton = (
    <Button type="submit" size="sm" disabled={pending || !title.trim()}>
      {submitLabel}
    </Button>
  );
  const fields = (
    <>
      <div className="flex flex-wrap gap-2">
        {expanded && (
          <select
            aria-label="Kind"
            className={cn(inputClass, "w-auto")}
            value={kind}
            onChange={(e) => setKind(e.target.value as MemoryKind)}
          >
            {kinds.map((option) => (
              <option key={option.kind} value={option.kind}>
                {option.label}
              </option>
            ))}
          </select>
        )}
        <input
          aria-label="Title"
          className={cn(inputClass, "min-w-48 flex-1", collapsible && "min-w-0")}
          placeholder={expanded ? "One line that states the point" : "New entry"}
          value={title}
          onChange={(e) => setTitle(e.target.value)}
        />
        {collapsible && submitButton}
      </div>
      {expanded && (
        <MarkdownEditor label="Body" value={body} onChange={setBody} placeholder="Why, context, alternatives" />
      )}
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {error.message}
        </p>
      )}
    </>
  );

  if (collapsible) {
    return (
      <CollapsibleForm label={label} expanded={expanded} onExpandedChange={setExpanded} onSubmit={submit}>
        {fields}
      </CollapsibleForm>
    );
  }

  return (
    <form
      aria-label={label}
      onSubmit={submit}
      onKeyDown={onCancel ? guard.onKeyDown : undefined}
      className="flex flex-col gap-2 rounded-lg border bg-muted/30 p-3"
    >
      {fields}
      {guard.asking ? (
        <DiscardConfirm
          label={`Discard ${label}`}
          question="Discard your changes to this entry?"
          onKeep={guard.keep}
          onDiscard={guard.discard}
        />
      ) : (
        <div className="flex gap-2">
          {submitButton}
          {onCancel && (
            <Button type="button" size="sm" variant="ghost" onClick={guard.request}>
              Cancel
            </Button>
          )}
        </div>
      )}
    </form>
  );
}
