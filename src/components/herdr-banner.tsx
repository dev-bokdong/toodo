import { Button } from "@/components/ui/button";
import { useHerdrStatus } from "@/lib/queries";

export function HerdrBanner() {
  const status = useHerdrStatus();
  if (status.isPending || status.data?.running) {
    return null;
  }
  const reason = status.isError ? status.error.message : "The herdr server is not running.";

  return (
    <div
      role="alert"
      className="mb-4 flex items-center justify-between gap-3 rounded-lg border border-amber-300 bg-amber-50 px-3 py-2 text-amber-900 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-100"
    >
      <div className="flex min-w-0 flex-col gap-0.5">
        <p className="text-sm font-medium">herdr is not available</p>
        <p className="text-xs break-words">
          {reason} Start herdr, then retry. Tasks cannot be started until herdr is running.
        </p>
      </div>
      <Button size="sm" variant="outline" onClick={() => status.refetch()} disabled={status.isFetching}>
        {status.isFetching ? "Checking..." : "Retry"}
      </Button>
    </div>
  );
}
