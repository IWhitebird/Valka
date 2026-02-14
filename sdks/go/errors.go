package valka

import "fmt"

// ValkaError is the base error type for all Valka SDK errors.
type ValkaError struct {
	Message string
	Cause   error
}

func (e *ValkaError) Error() string {
	if e.Cause != nil {
		return fmt.Sprintf("%s: %v", e.Message, e.Cause)
	}
	return e.Message
}

func (e *ValkaError) Unwrap() error { return e.Cause }

// ApiError represents an HTTP API error with a status code.
type ApiError struct {
	ValkaError
	Status int
}

func NewApiError(status int, message string) *ApiError {
	return &ApiError{
		ValkaError: ValkaError{Message: message},
		Status:     status,
	}
}

// ConnectionError represents a gRPC connection failure.
type ConnectionError struct {
	ValkaError
}

func NewConnectionError(message string, cause error) *ConnectionError {
	return &ConnectionError{
		ValkaError: ValkaError{Message: message, Cause: cause},
	}
}

// HandlerError represents a task handler failure.
type HandlerError struct {
	ValkaError
	Retryable bool
}

func NewHandlerError(message string, retryable bool) *HandlerError {
	return &HandlerError{
		ValkaError: ValkaError{Message: message},
		Retryable:  retryable,
	}
}

// NotConnectedError indicates the worker is not connected.
type NotConnectedError struct {
	ValkaError
}

func NewNotConnectedError() *NotConnectedError {
	return &NotConnectedError{
		ValkaError: ValkaError{Message: "worker is not connected"},
	}
}

// ShuttingDownError indicates the worker is shutting down.
type ShuttingDownError struct {
	ValkaError
}

func NewShuttingDownError() *ShuttingDownError {
	return &ShuttingDownError{
		ValkaError: ValkaError{Message: "worker is shutting down"},
	}
}
