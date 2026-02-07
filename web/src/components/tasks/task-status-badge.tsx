import { cn, statusColor, statusDotColor } from "@/lib/utils";

interface TaskStatusBadgeProps {
  status: string;
  className?: string;
}

export function TaskStatusBadge({ status, className }: TaskStatusBadgeProps) {
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-xs font-medium",
        statusColor(status),
        className,
      )}
    >
      <span className={cn("h-1.5 w-1.5 rounded-full", statusDotColor(status))} />
      {status}
    </span>
  );
}
