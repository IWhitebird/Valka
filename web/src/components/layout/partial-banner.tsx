import { useSyncExternalStore } from "react";
import { TriangleAlert } from "lucide-react";
import { partialAnswers } from "@/lib/partial";

/**
 * Shown while the latest cluster-wide answer of some list on this page came back without
 * some node's part (`X-Valka-Partial`); it goes away once those answers are whole again.
 */
export function PartialBanner() {
  const missing = useSyncExternalStore(partialAnswers.subscribe, partialAnswers.snapshot);
  if (missing === null) return null;
  return (
    <p
      role="status"
      className="mb-6 flex items-center gap-2 rounded-md border border-amber-500/30 bg-amber-500/5 px-3 py-2 text-xs text-amber-400"
    >
      <TriangleAlert className="h-3.5 w-3.5 shrink-0" />
      <span>
        {missing.length > 0 ? `Some nodes did not answer (${missing.join(", ")})` : "Some nodes did not answer"}
        : this page may be incomplete.
      </span>
    </p>
  );
}
