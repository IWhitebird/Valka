import { useState } from "react";
import { Search } from "lucide-react";
import { STATUS_OPTIONS } from "@/lib/utils";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

interface TaskFiltersProps {
  onFilter: (params: { queue_name?: string; status?: string }) => void;
  initialQueue?: string;
  initialStatus?: string;
}

export function TaskFilters({
  onFilter,
  initialQueue = "",
  initialStatus = "",
}: TaskFiltersProps) {
  const [queueName, setQueueName] = useState(initialQueue);
  const [status, setStatus] = useState(initialStatus);

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const resolvedStatus = status && status !== "__all__" ? status : undefined;
    onFilter({
      queue_name: queueName || undefined,
      status: resolvedStatus,
    });
  }

  return (
    <form onSubmit={handleSubmit} className="flex items-center gap-3">
      <Input
        type="text"
        placeholder="Filter by queue..."
        value={queueName}
        onChange={(e) => setQueueName(e.target.value)}
        className="w-48"
      />
      <Select value={status} onValueChange={setStatus}>
        <SelectTrigger className="w-44">
          <SelectValue placeholder="All Statuses" />
        </SelectTrigger>
        <SelectContent>
          {STATUS_OPTIONS.map((opt) => (
            <SelectItem key={opt.value} value={opt.value || "__all__"}>
              {opt.label}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <Button type="submit" variant="outline" size="default">
        <Search className="h-4 w-4" />
        Filter
      </Button>
    </form>
  );
}
