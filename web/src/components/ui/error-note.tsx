import { cn } from "@/lib/utils";

/**
 * A failed request or action, in the cluster pages' error style. `compact` is for a line next
 * to the button that failed.
 */
export function ErrorNote({
  children,
  compact = false,
  className,
}: {
  children: React.ReactNode;
  compact?: boolean;
  className?: string;
}) {
  return (
    <p
      role="alert"
      className={cn(
        "border border-red-500/30 bg-red-500/5 text-red-400",
        compact ? "rounded-md px-3 py-2 text-xs" : "rounded-lg p-4 text-sm",
        className,
      )}
    >
      {children}
    </p>
  );
}
