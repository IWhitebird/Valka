import {
  ListTodo,
  Clock,
  Play,
  CheckCircle,
  XCircle,
} from "lucide-react";
import type { Task } from "@/api/types";
import { cn } from "@/lib/utils";
import {
  Card,
  CardContent,
} from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";

interface StatsCardsProps {
  tasks: Task[];
  isLoading: boolean;
}

interface StatCardProps {
  title: string;
  value: number;
  icon: React.ComponentType<{ className?: string }>;
  iconColor: string;
  iconBg: string;
}

const stats = [
  {
    key: "total",
    title: "Total Tasks",
    icon: ListTodo,
    iconColor: "text-zinc-300",
    iconBg: "bg-zinc-500/15",
  },
  {
    key: "pending",
    title: "Pending",
    icon: Clock,
    iconColor: "text-zinc-400",
    iconBg: "bg-zinc-500/15",
  },
  {
    key: "running",
    title: "Running",
    icon: Play,
    iconColor: "text-sky-400",
    iconBg: "bg-sky-500/15",
  },
  {
    key: "completed",
    title: "Completed",
    icon: CheckCircle,
    iconColor: "text-emerald-400",
    iconBg: "bg-emerald-500/15",
  },
  {
    key: "failed",
    title: "Failed",
    icon: XCircle,
    iconColor: "text-red-400",
    iconBg: "bg-red-500/15",
  },
] as const;

function StatCard({ title, value, icon: Icon, iconColor, iconBg }: StatCardProps) {
  return (
    <Card className="gap-0 py-0">
      <CardContent className="flex items-center justify-between p-5">
        <div className="space-y-1">
          <p className="text-muted-foreground text-sm font-medium">{title}</p>
          <p className="text-2xl font-semibold tracking-tight">{value.toLocaleString()}</p>
        </div>
        <div className={cn("flex h-10 w-10 items-center justify-center rounded-lg", iconBg)}>
          <Icon className={cn("h-5 w-5", iconColor)} />
        </div>
      </CardContent>
    </Card>
  );
}

function StatCardSkeleton() {
  return (
    <Card className="gap-0 py-0">
      <CardContent className="flex items-center justify-between p-5">
        <div className="space-y-2">
          <Skeleton className="h-4 w-20" />
          <Skeleton className="h-7 w-12" />
        </div>
        <Skeleton className="h-10 w-10 rounded-lg" />
      </CardContent>
    </Card>
  );
}

export function StatsCards({ tasks, isLoading }: StatsCardsProps) {
  if (isLoading) {
    return (
      <div className="grid grid-cols-2 gap-4 lg:grid-cols-5">
        {Array.from({ length: 5 }).map((_, i) => (
          <StatCardSkeleton key={i} />
        ))}
      </div>
    );
  }

  const counts: Record<string, number> = {
    total: tasks.length,
    pending: tasks.filter((t) => t.status === "PENDING").length,
    running: tasks.filter((t) => t.status === "RUNNING").length,
    completed: tasks.filter((t) => t.status === "COMPLETED").length,
    failed: tasks.filter(
      (t) => t.status === "FAILED" || t.status === "DEAD_LETTER"
    ).length,
  };

  return (
    <div className="grid grid-cols-2 gap-4 lg:grid-cols-5">
      {stats.map((stat) => (
        <StatCard
          key={stat.key}
          title={stat.title}
          value={counts[stat.key]}
          icon={stat.icon}
          iconColor={stat.iconColor}
          iconBg={stat.iconBg}
        />
      ))}
    </div>
  );
}
