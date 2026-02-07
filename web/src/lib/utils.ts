import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";
import { format, formatDistanceToNow } from "date-fns";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

export function formatDate(date: string): string {
  return format(new Date(date), "MMM d, yyyy HH:mm:ss");
}

export function formatRelative(date: string): string {
  return formatDistanceToNow(new Date(date), { addSuffix: true });
}

export function truncateId(id: string, length = 8): string {
  return id.slice(0, length);
}

export const STATUS_OPTIONS = [
  { value: "", label: "All Statuses" },
  { value: "PENDING", label: "Pending" },
  { value: "DISPATCHING", label: "Dispatching" },
  { value: "RUNNING", label: "Running" },
  { value: "COMPLETED", label: "Completed" },
  { value: "FAILED", label: "Failed" },
  { value: "RETRY", label: "Retry" },
  { value: "DEAD_LETTER", label: "Dead Letter" },
  { value: "CANCELLED", label: "Cancelled" },
] as const;

export function statusColor(status: string): string {
  switch (status) {
    case "PENDING":
      return "bg-zinc-500/10 text-zinc-400 border-zinc-500/20";
    case "DISPATCHING":
      return "bg-blue-500/10 text-blue-400 border-blue-500/20";
    case "RUNNING":
      return "bg-sky-500/10 text-sky-400 border-sky-500/20";
    case "COMPLETED":
      return "bg-emerald-500/10 text-emerald-400 border-emerald-500/20";
    case "FAILED":
      return "bg-red-500/10 text-red-400 border-red-500/20";
    case "RETRY":
      return "bg-amber-500/10 text-amber-400 border-amber-500/20";
    case "DEAD_LETTER":
      return "bg-rose-500/10 text-rose-400 border-rose-500/20";
    case "CANCELLED":
      return "bg-neutral-500/10 text-neutral-400 border-neutral-500/20";
    default:
      return "bg-zinc-500/10 text-zinc-400 border-zinc-500/20";
  }
}

export function statusDotColor(status: string): string {
  switch (status) {
    case "PENDING":
      return "bg-zinc-400";
    case "DISPATCHING":
      return "bg-blue-400";
    case "RUNNING":
      return "bg-sky-400";
    case "COMPLETED":
      return "bg-emerald-400";
    case "FAILED":
      return "bg-red-400";
    case "RETRY":
      return "bg-amber-400";
    case "DEAD_LETTER":
      return "bg-rose-400";
    case "CANCELLED":
      return "bg-neutral-400";
    default:
      return "bg-zinc-400";
  }
}
