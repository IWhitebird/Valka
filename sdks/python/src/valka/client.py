"""Valka REST API client."""

from __future__ import annotations

from typing import Any

import httpx

from valka.errors import ApiError
from valka.types import (
    CreateTaskOptions,
    DeadLetter,
    GetRunLogsOptions,
    ListDeadLettersOptions,
    ListTasksOptions,
    Task,
    TaskLog,
    TaskRun,
    WorkerInfo,
)


class ValkaClient:
    """Async REST client for the Valka task queue API.

    Usage::

        async with ValkaClient("http://localhost:8080") as client:
            task = await client.create_task(
                queue_name="emails",
                task_name="send-welcome",
                input={"to": "user@example.com"},
            )
    """

    def __init__(
        self,
        base_url: str = "http://localhost:8080",
        *,
        headers: dict[str, str] | None = None,
    ) -> None:
        self._base_url = base_url.rstrip("/")
        self._client = httpx.AsyncClient(
            base_url=f"{self._base_url}/api/v1",
            headers=headers or {},
        )

    async def __aenter__(self) -> ValkaClient:
        return self

    async def __aexit__(self, *exc: object) -> None:
        await self.close()

    async def close(self) -> None:
        """Close the underlying HTTP client."""
        await self._client.aclose()

    # -- Task CRUD --

    async def create_task(
        self,
        queue_name: str,
        task_name: str,
        input: Any = None,
        **kwargs: Any,
    ) -> Task:
        """Create a new task."""
        body: CreateTaskOptions = {
            "queue_name": queue_name,
            "task_name": task_name,
        }
        if input is not None:
            body["input"] = input
        for key in (
            "priority",
            "max_retries",
            "timeout_seconds",
            "idempotency_key",
            "metadata",
            "scheduled_at",
        ):
            if key in kwargs:
                body[key] = kwargs[key]  # type: ignore[literal-required]
        return await self._post("/tasks", body)

    async def get_task(self, task_id: str) -> Task:
        """Get a task by ID."""
        return await self._get(f"/tasks/{task_id}")

    async def list_tasks(
        self,
        queue_name: str | None = None,
        status: str | None = None,
        limit: int = 50,
        offset: int = 0,
    ) -> list[Task]:
        """List tasks with optional filters."""
        params: dict[str, Any] = {"limit": limit, "offset": offset}
        if queue_name is not None:
            params["queue_name"] = queue_name
        if status is not None:
            params["status"] = status
        return await self._get("/tasks", params=params)

    async def cancel_task(self, task_id: str) -> Task:
        """Cancel a task."""
        return await self._post(f"/tasks/{task_id}/cancel")

    # -- Runs & Logs --

    async def get_task_runs(self, task_id: str) -> list[TaskRun]:
        """Get execution attempts for a task."""
        return await self._get(f"/tasks/{task_id}/runs")

    async def get_run_logs(
        self,
        task_id: str,
        run_id: str,
        limit: int = 1000,
        after_id: str | None = None,
    ) -> list[TaskLog]:
        """Get logs for a specific task run."""
        params: dict[str, Any] = {"limit": limit}
        if after_id is not None:
            params["after_id"] = after_id
        return await self._get(f"/tasks/{task_id}/runs/{run_id}/logs", params=params)

    # -- Workers --

    async def list_workers(self) -> list[WorkerInfo]:
        """List connected workers."""
        return await self._get("/workers")

    # -- Dead Letters --

    async def list_dead_letters(
        self,
        queue_name: str | None = None,
        limit: int = 50,
        offset: int = 0,
    ) -> list[DeadLetter]:
        """List dead-lettered tasks."""
        params: dict[str, Any] = {"limit": limit, "offset": offset}
        if queue_name is not None:
            params["queue_name"] = queue_name
        return await self._get("/dead-letters", params=params)

    # -- Health --

    async def health_check(self) -> str:
        """Check server health. Returns 'ok' on success."""
        resp = await self._client.get(f"{self._base_url}/healthz")
        resp.raise_for_status()
        return resp.text

    # -- Internal helpers --

    async def _get(self, path: str, *, params: dict[str, Any] | None = None) -> Any:
        resp = await self._client.get(path, params=params)
        if resp.status_code >= 400:
            raise ApiError(resp.text, status=resp.status_code)
        return resp.json()

    async def _post(self, path: str, body: Any = None) -> Any:
        if body is not None:
            resp = await self._client.post(path, json=body)
        else:
            resp = await self._client.post(path)
        if resp.status_code >= 400:
            raise ApiError(resp.text, status=resp.status_code)
        return resp.json()
