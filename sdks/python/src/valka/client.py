"""Valka REST API client."""

from __future__ import annotations

import asyncio
import os
import uuid
from typing import Any

import httpx

from valka.errors import ApiError
from valka.types import (
    CreateTaskOptions,
    DeadLetter,
    Task,
    TaskLog,
    TaskRun,
    WorkerInfo,
)

# Answers that may succeed on a retry, possibly on another node.
_RETRYABLE_STATUS = {429, 502, 503, 504}


class ValkaClient:
    """Async REST client for the Valka task queue API.

    Give it every node's URL (``base_urls``): any node takes any request, so when one
    goes away the client moves to the next. Creates carry an idempotency key (generated
    when you set none), so a create retried after a lost response returns the task it
    already made. Reads are retried the same way; cancels and signals are not.

    Usage::

        async with ValkaClient("http://localhost:8989", api_key="...") as client:
            task = await client.create_task(
                queue_name="emails",
                task_name="send-welcome",
                input={"to": "user@example.com"},
            )
    """

    def __init__(
        self,
        base_url: str = "http://localhost:8989",
        *,
        base_urls: list[str] | None = None,
        api_key: str | None = None,
        headers: dict[str, str] | None = None,
        attempts: int = 6,
        timeout: float = 10.0,
        auto_idempotency: bool = True,
    ) -> None:
        urls = [base_url, *(base_urls or [])]
        self._base_urls = [u.rstrip("/") for u in urls]
        self._base_url = self._base_urls[0]
        self._next = 0
        self._attempts = max(1, attempts)
        self._auto_idempotency = auto_idempotency
        hdrs = dict(headers or {})
        key = api_key if api_key is not None else os.environ.get("VALKA_API_KEY", "")
        if key:
            hdrs["Authorization"] = f"Bearer {key}"
        self._client = httpx.AsyncClient(headers=hdrs, timeout=timeout)

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
        if not body.get("idempotency_key") and self._auto_idempotency:
            body["idempotency_key"] = f"auto-{uuid.uuid4().hex}"
        return await self._request(
            "POST", "/tasks", json=body, retry=bool(body.get("idempotency_key"))
        )

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
        """Cancel a task (sent once: a cancel is not idempotent)."""
        return await self._request("POST", f"/tasks/{task_id}/cancel", retry=False)

    # -- Runs & Logs --

    async def get_task_runs(self, task_id: str) -> list[TaskRun]:
        """Get execution attempts for a task."""
        return await self._get(f"/tasks/{task_id}/runs")

    async def get_run_logs(
        self,
        task_id: str,
        run_id: str,
        limit: int = 1000,
        after_id: int | str | None = None,
    ) -> list[TaskLog]:
        """Get logs for a specific task run (``after_id``: only entries after this log id)."""
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

    # -- Signals --

    async def send_signal(
        self,
        task_id: str,
        signal_name: str,
        payload: Any = None,
    ) -> dict[str, Any]:
        """Send a named signal to a task. Returns {signal_id, delivered}."""
        body: dict[str, Any] = {"signal_name": signal_name}
        if payload is not None:
            body["payload"] = payload
        return await self._request("POST", f"/tasks/{task_id}/signal", json=body, retry=False)

    async def list_signals(
        self,
        task_id: str,
        status: str | None = None,
    ) -> list[dict[str, Any]]:
        """List signals for a task, optionally filtered by status."""
        params: dict[str, Any] = {}
        if status is not None:
            params["status"] = status
        return await self._get(f"/tasks/{task_id}/signals", params=params or None)

    # -- Health --

    async def health_check(self) -> str:
        """Check server health. Returns 'ok' on success."""
        resp = await self._send("GET", "/healthz", retry=True)
        return resp.text

    # -- Internal helpers --

    async def _get(self, path: str, *, params: dict[str, Any] | None = None) -> Any:
        return await self._request("GET", path, params=params, retry=True)

    async def _request(
        self,
        method: str,
        path: str,
        *,
        params: dict[str, Any] | None = None,
        json: Any = None,
        retry: bool,
    ) -> Any:
        resp = await self._send(method, f"/api/v1{path}", params=params, json=json, retry=retry)
        return resp.json()

    async def _send(
        self,
        method: str,
        path: str,
        *,
        params: dict[str, Any] | None = None,
        json: Any = None,
        retry: bool,
    ) -> httpx.Response:
        """Send one request; a retryable failure moves to the next node and tries again."""
        attempts = self._attempts if retry else 1
        delay = 0.1
        last: Exception | None = None
        for attempt in range(attempts):
            if attempt:
                await asyncio.sleep(delay)
                delay = min(delay * 2, 3.0)
            node = self._next
            url = self._base_urls[node % len(self._base_urls)] + path
            try:
                resp = await self._client.request(method, url, params=params, json=json)
            except httpx.TransportError as exc:
                last = exc
                self._advance(node)
                continue
            if resp.status_code >= 400:
                last = ApiError(resp.text, status=resp.status_code)
                if resp.status_code in _RETRYABLE_STATUS:
                    self._advance(node)
                    continue
                raise last
            return resp
        assert last is not None
        raise last

    def _advance(self, node: int) -> None:
        if self._next == node:
            self._next = node + 1
