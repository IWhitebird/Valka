import type { TaskCheckpoint } from "@/api/types";
import { truncateId, formatDate, formatJsonValue } from "@/lib/utils";
import {
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
} from "@/components/ui/table";

interface TaskCheckpointsTableProps {
  checkpoints: TaskCheckpoint[];
}

const headClass = "px-4 text-xs uppercase tracking-wider text-muted-foreground";

export function TaskCheckpointsTable({ checkpoints }: TaskCheckpointsTableProps) {
  return (
    <div className="overflow-hidden rounded-lg border">
      <Table>
        <TableHeader>
          <TableRow className="hover:bg-transparent">
            <TableHead className={headClass}>Step</TableHead>
            <TableHead className={headClass}>Attempt</TableHead>
            <TableHead className={headClass}>Run ID</TableHead>
            <TableHead className={headClass}>Recorded</TableHead>
            <TableHead className={headClass}>Output</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {checkpoints.map((checkpoint) => {
            const output = formatJsonValue(checkpoint.output);
            return (
              <TableRow key={checkpoint.step}>
                <TableCell className="px-4 font-medium">{checkpoint.step}</TableCell>
                <TableCell className="px-4">#{checkpoint.attempt_number}</TableCell>
                <TableCell className="px-4 font-mono text-xs text-muted-foreground">
                  {truncateId(checkpoint.run_id)}
                </TableCell>
                <TableCell className="px-4 text-xs text-muted-foreground">
                  {formatDate(checkpoint.created_at)}
                </TableCell>
                <TableCell
                  className="max-w-[320px] truncate px-4 font-mono text-xs"
                  title={output}
                >
                  {output}
                </TableCell>
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
    </div>
  );
}
