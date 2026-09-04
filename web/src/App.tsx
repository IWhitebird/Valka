import { BrowserRouter, Routes, Route } from "react-router-dom";
import { QueryClientProvider } from "@tanstack/react-query";
import { queryClient } from "@/lib/query-client";
import { RootLayout } from "@/components/layout/root-layout";
import { DashboardPage } from "@/pages/dashboard";
import { TasksPage } from "@/pages/tasks";
import { TaskDetailPage } from "@/pages/task-detail";
import { WorkersPage } from "@/pages/workers";
import { EventsPage } from "@/pages/events";
import { DeadLettersPage } from "@/pages/dead-letters";
import { ClusterPage } from "@/pages/cluster";
import { ClusterNodePage } from "@/pages/cluster-node";
import { ClusterShardsPage } from "@/pages/cluster-shards";
import { ClusterStoragePage } from "@/pages/cluster-storage";

function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <BrowserRouter>
        <Routes>
          <Route element={<RootLayout />}>
            <Route path="/" element={<DashboardPage />} />
            <Route path="/tasks" element={<TasksPage />} />
            <Route path="/tasks/:taskId" element={<TaskDetailPage />} />
            <Route path="/workers" element={<WorkersPage />} />
            <Route path="/cluster" element={<ClusterPage />} />
            <Route path="/cluster/nodes/:nodeId" element={<ClusterNodePage />} />
            <Route path="/cluster/shards" element={<ClusterShardsPage />} />
            <Route path="/cluster/storage" element={<ClusterStoragePage />} />
            <Route path="/events" element={<EventsPage />} />
            <Route path="/dead-letters" element={<DeadLettersPage />} />
          </Route>
        </Routes>
      </BrowserRouter>
    </QueryClientProvider>
  );
}

export default App;
