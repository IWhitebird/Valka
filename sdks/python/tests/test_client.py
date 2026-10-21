"""ValkaClient: node failover, API keys and safe create retries (no server needed)."""

from __future__ import annotations

import asyncio
import json

import httpx
import pytest

from valka.client import ValkaClient
from valka.errors import ApiError


def _client(handler, **kwargs) -> ValkaClient:
    client = ValkaClient("http://down:1", base_urls=["http://up:2"], api_key="k1", **kwargs)
    client._client = httpx.AsyncClient(
        transport=httpx.MockTransport(handler), headers=client._client.headers
    )
    return client


def test_create_moves_to_the_next_node_with_the_same_key() -> None:
    seen: list[tuple[str, str, str]] = []

    def handler(request: httpx.Request) -> httpx.Response:
        body = json.loads(request.content)
        seen.append((request.url.host, body["idempotency_key"], request.headers["authorization"]))
        if request.url.host == "down":
            return httpx.Response(503, text="node is down")
        return httpx.Response(201, json={"id": "t1", "status": "PENDING"})

    async def go() -> None:
        client = _client(handler)
        task = await client.create_task("q", "t", input={"n": 1})
        assert task["id"] == "t1"
        await client.close()

    asyncio.run(go())
    assert [s[0] for s in seen] == ["down", "up"]
    assert seen[0][1] == seen[1][1] and seen[0][1].startswith("auto-")
    assert all(s[2] == "Bearer k1" for s in seen)


def test_cancel_is_sent_once() -> None:
    calls = 0

    def handler(request: httpx.Request) -> httpx.Response:
        nonlocal calls
        calls += 1
        return httpx.Response(503, text="node is down")

    async def go() -> None:
        client = _client(handler)
        with pytest.raises(ApiError) as err:
            await client.cancel_task("t1")
        assert err.value.status == 503
        await client.close()

    asyncio.run(go())
    assert calls == 1


def test_client_errors_are_not_retried() -> None:
    calls = 0

    def handler(request: httpx.Request) -> httpx.Response:
        nonlocal calls
        calls += 1
        return httpx.Response(404, text="Task not found")

    async def go() -> None:
        client = _client(handler)
        with pytest.raises(ApiError):
            await client.get_task("t1")
        await client.close()

    asyncio.run(go())
    assert calls == 1


def test_auto_idempotency_can_be_turned_off() -> None:
    bodies: list[dict] = []

    def handler(request: httpx.Request) -> httpx.Response:
        bodies.append(json.loads(request.content))
        return httpx.Response(201, json={"id": "t1"})

    async def go() -> None:
        client = _client(handler, auto_idempotency=False)
        await client.create_task("q", "t")
        await client.close()

    asyncio.run(go())
    assert "idempotency_key" not in bodies[0]
