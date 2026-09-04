import type { StorageStats } from "@/api/types";
import { Stat } from "@/components/cluster/stat";
import { formatBytes, formatDurationSecs } from "@/lib/cluster";

export function StorageCards({ storage }: { storage: StorageStats }) {
  return (
    <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-4">
      <Stat
        label="WAL segments"
        value={storage.wal.segments.toLocaleString()}
        hint={`${formatBytes(storage.wal.bytes)} · oldest ${formatDurationSecs(storage.wal.oldest_age_secs)}`}
        tone={storage.wal.segments > 500 ? "warn" : "default"}
      />
      <Stat
        label="Snapshots"
        value={storage.snapshots.count.toLocaleString()}
        hint={`${formatBytes(storage.snapshots.bytes)} · oldest ${formatDurationSecs(storage.snapshots.oldest_age_secs)}`}
      />
      <Stat label="Log chunks" value={storage.logs.chunks.toLocaleString()} hint={formatBytes(storage.logs.bytes)} />
      <Stat
        label="Est. cost ceiling"
        value={`$${storage.estimate.max_usd_per_day.toFixed(2)}/day`}
        hint={`≤ ${storage.estimate.max_puts_per_day.toLocaleString()} PUTs/day at a ${storage.estimate.flush_interval_ms} ms flush window`}
      />
    </div>
  );
}
