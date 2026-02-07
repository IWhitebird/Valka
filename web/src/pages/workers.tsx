import { useQuery } from "@tanstack/react-query";
import { RefreshCw } from "lucide-react";
import { workersApi } from "@/api/workers";
import { WorkerTable } from "@/components/workers/worker-table";
import { Card, CardContent } from "@/components/ui/card";
import { Button } from "@/components/ui/button";

export function WorkersPage() {
  const {
    data: workers = [],
    isLoading,
    refetch,
  } = useQuery({
    queryKey: ["workers"],
    queryFn: workersApi.list,
    refetchInterval: 5_000,
  });

  const totalActiveTasks = workers.reduce((sum, w) => sum + w.active_tasks, 0);
  const totalCapacity = workers.reduce((sum, w) => sum + w.concurrency, 0);

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight text-foreground">Workers</h1>
          <p className="mt-1 text-sm text-muted-foreground">
            Connected workers and their status
          </p>
        </div>
        <Button variant="outline" size="icon" onClick={() => refetch()}>
          <RefreshCw className="h-4 w-4" />
        </Button>
      </div>

      <div className="grid grid-cols-3 gap-4">
        <Card className="gap-0 py-0">
          <CardContent className="p-5">
            <p className="text-sm text-muted-foreground">Total Workers</p>
            <p className="mt-1 text-2xl font-semibold tracking-tight text-foreground">
              {workers.length}
            </p>
          </CardContent>
        </Card>
        <Card className="gap-0 py-0">
          <CardContent className="p-5">
            <p className="text-sm text-muted-foreground">Active Tasks</p>
            <p className="mt-1 text-2xl font-semibold tracking-tight text-foreground">
              {totalActiveTasks}
            </p>
          </CardContent>
        </Card>
        <Card className="gap-0 py-0">
          <CardContent className="p-5">
            <p className="text-sm text-muted-foreground">Total Capacity</p>
            <p className="mt-1 text-2xl font-semibold tracking-tight text-foreground">
              {totalCapacity}
            </p>
          </CardContent>
        </Card>
      </div>

      <WorkerTable workers={workers} isLoading={isLoading} />
    </div>
  );
}
