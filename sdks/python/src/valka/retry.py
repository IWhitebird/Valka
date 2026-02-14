"""Exponential backoff retry policy with jitter."""

from __future__ import annotations

import random


class RetryPolicy:
    """Exponential backoff with jitter for reconnection."""

    def __init__(
        self,
        initial_delay_ms: float = 100,
        max_delay_ms: float = 30_000,
        multiplier: float = 2.0,
    ) -> None:
        self.initial_delay_ms = initial_delay_ms
        self.max_delay_ms = max_delay_ms
        self.multiplier = multiplier
        self._attempt = 0

    def next_delay(self) -> float:
        """Calculate and return the next delay in milliseconds."""
        delay = self.initial_delay_ms * (self.multiplier ** self._attempt)
        capped = min(delay, self.max_delay_ms)
        jitter = capped * 0.1 * random.random()
        self._attempt += 1
        return capped + jitter

    def next_delay_seconds(self) -> float:
        """Calculate and return the next delay in seconds."""
        return self.next_delay() / 1000.0

    def reset(self) -> None:
        """Reset the attempt counter (call on successful connection)."""
        self._attempt = 0
