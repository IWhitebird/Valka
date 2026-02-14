"""Valka SDK error types."""


class ValkaError(Exception):
    """Base error for all Valka SDK errors."""


class ApiError(ValkaError):
    """HTTP API error with status code."""

    def __init__(self, message: str, status: int) -> None:
        super().__init__(message)
        self.status = status


class ConnectionError(ValkaError):
    """gRPC connection failure."""


class HandlerError(ValkaError):
    """Task handler failure with retryable flag."""

    def __init__(self, message: str, *, retryable: bool = True) -> None:
        super().__init__(message)
        self.retryable = retryable


class NotConnectedError(ValkaError):
    """Worker is not connected to the server."""

    def __init__(self) -> None:
        super().__init__("Worker is not connected")


class ShuttingDownError(ValkaError):
    """Worker is shutting down."""

    def __init__(self) -> None:
        super().__init__("Worker is shutting down")
